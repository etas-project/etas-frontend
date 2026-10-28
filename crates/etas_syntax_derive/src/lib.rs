use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, parse_macro_input};

/// Enumerates every AST field. Traversal and span changes remain iterative in
/// etas_syntax, so recursive syntax never consumes the Rust call stack here.
#[proc_macro_derive(SpanNodes)]
pub fn span_nodes(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let body = |mutable: bool| -> syn::Result<proc_macro2::TokenStream> {
        let push = |value: proc_macro2::TokenStream| {
            if mutable {
                quote!(work.push(&mut #value);)
            } else {
                quote!(work.push(&#value);)
            }
        };
        Ok(match &input.data {
            Data::Struct(data) => {
                let fields = data.fields.iter().enumerate().map(|(index, field)| {
                    let member = field
                        .ident
                        .as_ref()
                        .map(|name| quote!(#name))
                        .unwrap_or_else(|| {
                            let index = syn::Index::from(index);
                            quote!(#index)
                        });
                    push(quote!(self.#member))
                });
                quote!(#(#fields)*)
            }
            Data::Enum(data) => {
                let arms = data.variants.iter().map(|variant| {
                    let variant_name = &variant.ident;
                    let bindings = (0..variant.fields.len())
                        .map(|index| format_ident!("field_{index}"))
                        .collect::<Vec<_>>();
                    let pattern = match &variant.fields {
                        Fields::Named(fields) => {
                            let names = fields.named.iter().map(|field| &field.ident);
                            quote!(Self::#variant_name { #(#names: #bindings),* })
                        }
                        Fields::Unnamed(_) => quote!(Self::#variant_name(#(#bindings),*)),
                        Fields::Unit => quote!(Self::#variant_name),
                    };
                    quote!(#pattern => { #(work.push(#bindings);)* })
                });
                quote!(match self { #(#arms),* })
            }
            Data::Union(_) => {
                return Err(syn::Error::new_spanned(
                    name,
                    "SpanNodes cannot derive for unions",
                ));
            }
        })
    };
    let shared = body(false)?;
    let mutable = body(true)?;
    Ok(quote! {
        impl #impl_generics crate::source_mapping::SpanNode for #name #ty_generics #where_clause {
            fn children<'a>(&'a self, work: &mut crate::source_mapping::SpanWork<'a>) { #shared }
            fn children_mut<'a>(&'a mut self, work: &mut Vec<&'a mut dyn crate::source_mapping::SpanNode>) { #mutable }
        }
    })
}
