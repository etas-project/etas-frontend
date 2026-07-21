use crate::{NumericLiteralKind, PrimitiveType, Type, TypeId, TypeStore};

const ETAS_POINTER_BITS: u32 = 64;

pub(super) fn validate(
    store: &TypeStore,
    kind: NumericLiteralKind,
    text: &str,
    ty: TypeId,
) -> Result<(), String> {
    let Some(Type::Primitive(primitive)) = store.get(ty) else {
        return Err("the target is not a primitive numeric type".to_owned());
    };
    match kind {
        NumericLiteralKind::Integer => validate_integer(text, *primitive),
        NumericLiteralKind::Float => validate_float(text, *primitive),
    }
}

fn validate_integer(text: &str, primitive: PrimitiveType) -> Result<(), String> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (radix, digits) = if let Some(digits) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        (16, digits)
    } else if let Some(digits) = digits
        .strip_prefix("0o")
        .or_else(|| digits.strip_prefix("0O"))
    {
        (8, digits)
    } else if let Some(digits) = digits
        .strip_prefix("0b")
        .or_else(|| digits.strip_prefix("0B"))
    {
        (2, digits)
    } else {
        (10, digits)
    };
    let digits = normalized_digits(digits, radix)?;
    let magnitude = u128::from_str_radix(&digits, radix)
        .map_err(|_| "the integer magnitude exceeds 128 bits".to_owned())?;

    let (signed, bits) = integer_layout(primitive)
        .ok_or_else(|| "an integer literal requires an integer target type".to_owned())?;
    if !signed {
        if negative {
            return Err(format!(
                "a negative literal is not representable as {}",
                primitive.source_name()
            ));
        }
        let maximum = if bits == 128 {
            u128::MAX
        } else {
            (1_u128 << bits) - 1
        };
        return (magnitude <= maximum).then_some(()).ok_or_else(|| {
            format!(
                "the value is outside the range of {}",
                primitive.source_name()
            )
        });
    }

    let maximum = (1_u128 << (bits - 1)) - 1;
    let maximum_magnitude = if negative { maximum + 1 } else { maximum };
    (magnitude <= maximum_magnitude)
        .then_some(())
        .ok_or_else(|| {
            format!(
                "the value is outside the range of {}",
                primitive.source_name()
            )
        })
}

fn validate_float(text: &str, primitive: PrimitiveType) -> Result<(), String> {
    if !matches!(primitive, PrimitiveType::F32 | PrimitiveType::F64) {
        return Err("a floating-point literal requires an f32 or f64 target type".to_owned());
    }
    let normalized = normalized_float(text)?;
    let value = normalized
        .parse::<f64>()
        .map_err(|_| "the floating-point literal is malformed".to_owned())?;
    if !value.is_finite() {
        return Err("the value is not finite".to_owned());
    }
    if value == 0.0 && float_mantissa_is_nonzero(&normalized) {
        return Err(format!(
            "the value is outside the representable range of {}",
            primitive.source_name()
        ));
    }
    if primitive == PrimitiveType::F32 {
        let narrowed = value as f32;
        if !narrowed.is_finite() || (value != 0.0 && narrowed == 0.0) {
            return Err("the value is outside the representable range of f32".to_owned());
        }
    }
    Ok(())
}

fn float_mantissa_is_nonzero(text: &str) -> bool {
    text.trim_start_matches(['+', '-'])
        .split(['e', 'E'])
        .next()
        .is_some_and(|mantissa| mantissa.chars().any(|ch| ch.is_ascii_digit() && ch != '0'))
}

fn integer_layout(primitive: PrimitiveType) -> Option<(bool, u32)> {
    Some(match primitive {
        PrimitiveType::I8 => (true, 8),
        PrimitiveType::I16 => (true, 16),
        PrimitiveType::I32 => (true, 32),
        PrimitiveType::I64 => (true, 64),
        PrimitiveType::I128 => (true, 128),
        PrimitiveType::ISize => (true, ETAS_POINTER_BITS),
        PrimitiveType::U8 => (false, 8),
        PrimitiveType::U16 => (false, 16),
        PrimitiveType::U32 => (false, 32),
        PrimitiveType::U64 => (false, 64),
        PrimitiveType::U128 => (false, 128),
        PrimitiveType::USize => (false, ETAS_POINTER_BITS),
        _ => return None,
    })
}

fn normalized_digits(digits: &str, radix: u32) -> Result<String, String> {
    if digits.is_empty() {
        return Err("the integer literal has no digits".to_owned());
    }
    let chars = digits.chars().collect::<Vec<_>>();
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch == '_' {
            if index == 0
                || index + 1 == chars.len()
                || chars[index - 1] == '_'
                || chars[index + 1] == '_'
            {
                return Err("numeric separators must appear between digits".to_owned());
            }
        } else if ch.to_digit(radix).is_none() {
            return Err(format!("`{ch}` is not a base-{radix} digit"));
        }
    }
    Ok(chars.into_iter().filter(|ch| *ch != '_').collect())
}

fn normalized_float(text: &str) -> Result<String, String> {
    let chars = text.chars().collect::<Vec<_>>();
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch == '_'
            && (index == 0
                || index + 1 == chars.len()
                || !chars[index - 1].is_ascii_digit()
                || !chars[index + 1].is_ascii_digit())
        {
            return Err("numeric separators must appear between digits".to_owned());
        }
    }
    Ok(chars.into_iter().filter(|ch| *ch != '_').collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_integer_widths_radices_and_signed_minimum() {
        assert!(validate_integer("255", PrimitiveType::U8).is_ok());
        assert!(validate_integer("0xff", PrimitiveType::U8).is_ok());
        assert!(validate_integer("0b1111_1111", PrimitiveType::U8).is_ok());
        assert!(validate_integer("256", PrimitiveType::U8).is_err());
        assert!(validate_integer("-128", PrimitiveType::I8).is_ok());
        assert!(validate_integer("-129", PrimitiveType::I8).is_err());
        assert!(validate_integer("128", PrimitiveType::I8).is_err());

        for (primitive, maximum, overflow) in [
            (PrimitiveType::I16, "32767", "32768"),
            (PrimitiveType::I32, "2147483647", "2147483648"),
            (
                PrimitiveType::I64,
                "9223372036854775807",
                "9223372036854775808",
            ),
            (
                PrimitiveType::I128,
                "170141183460469231731687303715884105727",
                "170141183460469231731687303715884105728",
            ),
            (
                PrimitiveType::ISize,
                "9223372036854775807",
                "9223372036854775808",
            ),
            (PrimitiveType::U16, "65535", "65536"),
            (PrimitiveType::U32, "4294967295", "4294967296"),
            (
                PrimitiveType::U64,
                "18446744073709551615",
                "18446744073709551616",
            ),
            (
                PrimitiveType::U128,
                "340282366920938463463374607431768211455",
                "340282366920938463463374607431768211456",
            ),
            (
                PrimitiveType::USize,
                "18446744073709551615",
                "18446744073709551616",
            ),
        ] {
            assert!(
                validate_integer(maximum, primitive).is_ok(),
                "{primitive:?}"
            );
            assert!(
                validate_integer(overflow, primitive).is_err(),
                "{primitive:?}"
            );
        }
    }

    #[test]
    fn validates_float_target_range() {
        assert!(validate_float("3.5", PrimitiveType::F32).is_ok());
        assert!(validate_float("1e39", PrimitiveType::F32).is_err());
        assert!(validate_float("1e309", PrimitiveType::F64).is_err());
        assert!(validate_float("1e-400", PrimitiveType::F64).is_err());
    }
}
