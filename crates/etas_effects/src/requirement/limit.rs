#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct LimitRequirement {
    pub kind: LimitKind,
    pub budget: LimitBudgetKind,
    pub value: Option<LimitValue>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum LimitValue {
    Count(u64),
    MoneyMicros { amount: u128, currency: String },
    DurationMillis(u64),
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum LimitKind {
    Iterations,
    Tokens,
    ContextTokens,
    Cost,
    WallTime,
    Attempts,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum LimitBudgetKind {
    Count,
    Money,
    Duration,
}

impl LimitKind {
    pub const fn budget_kind(self) -> LimitBudgetKind {
        match self {
            Self::Iterations | Self::Tokens | Self::ContextTokens | Self::Attempts => {
                LimitBudgetKind::Count
            }
            Self::Cost => LimitBudgetKind::Money,
            Self::WallTime => LimitBudgetKind::Duration,
        }
    }
}
