//! Implementation of `#[derive(AppError)]`.
//!
//! Generates `Display` and `IntoResponse` implementations for error enums,
//! reading the optional `#[error(status = NNN, message = "...")]` attribute
//! on each variant.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse2, Data, DeriveInput, Fields, LitInt, LitStr, Token, Variant,
};

/// A single `key = value` entry inside `#[error(...)]`.
enum ErrorField {
    Status(u16),
    Message(String),
}

impl Parse for ErrorField {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        if ident == "status" {
            let lit: LitInt = input.parse()?;
            Ok(ErrorField::Status(lit.base10_parse()?))
        } else if ident == "message" {
            let lit: LitStr = input.parse()?;
            Ok(ErrorField::Message(lit.value()))
        } else {
            Err(syn::Error::new(
                ident.span(),
                "expected `status` or `message`",
            ))
        }
    }
}

/// The resolved `status`/`message` pair for one variant.
struct VariantAttr {
    status: u16,
    message: String,
}

fn variant_attr(variant: &Variant) -> syn::Result<VariantAttr> {
    let mut status = 500u16;
    let mut message = variant.ident.to_string();

    for attr in &variant.attrs {
        if !attr.path().is_ident("error") {
            continue;
        }
        let fields = attr.parse_args_with(
            syn::punctuated::Punctuated::<ErrorField, Token![,]>::parse_terminated,
        )?;
        for field in fields {
            match field {
                ErrorField::Status(s) => status = s,
                ErrorField::Message(m) => message = m,
            }
        }
    }

    Ok(VariantAttr { status, message })
}

/// Builds the match pattern for a variant, ignoring any payload.
fn variant_pattern(enum_ident: &Ident, variant: &Variant) -> TokenStream {
    let variant_ident = &variant.ident;
    match &variant.fields {
        Fields::Unit => quote! { #enum_ident::#variant_ident },
        Fields::Unnamed(_) => quote! { #enum_ident::#variant_ident(..) },
        Fields::Named(_) => quote! { #enum_ident::#variant_ident { .. } },
    }
}

/// Generates `Display` and `IntoResponse` impls for the given `#[derive(AppError)]` input.
pub fn derive(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match parse2(input) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    let enum_ident = input.ident.clone();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let data_enum = match &input.data {
        Data::Enum(data_enum) => data_enum,
        _ => {
            return syn::Error::new(
                Span::call_site(),
                "#[derive(AppError)] can only be used on enums",
            )
            .to_compile_error();
        }
    };

    let mut display_arms = Vec::new();
    let mut response_arms = Vec::new();

    for variant in &data_enum.variants {
        let attr = match variant_attr(variant) {
            Ok(attr) => attr,
            Err(err) => return err.to_compile_error(),
        };
        let pattern = variant_pattern(&enum_ident, variant);
        let message = &attr.message;
        let status = attr.status;

        display_arms.push(quote! {
            #pattern => ::std::write!(f, "{}", #message),
        });

        response_arms.push(quote! {
            #pattern => ::ladoo::response::IntoResponse::into_response(
                ::ladoo::error::Error::new(
                    ::http::StatusCode::from_u16(#status).unwrap(),
                    #message,
                ),
            ),
        });
    }

    quote! {
        impl #impl_generics ::std::fmt::Display for #enum_ident #ty_generics #where_clause {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                match self {
                    #(#display_arms)*
                }
            }
        }

        impl #impl_generics ::ladoo::response::IntoResponse for #enum_ident #ty_generics #where_clause {
            fn into_response(self) -> ::ladoo::response::Response {
                match self {
                    #(#response_arms)*
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_structs() {
        let input: TokenStream = quote! {
            struct NotAnEnum;
        };
        let output = derive(input);
        assert!(output.to_string().contains("compile_error"));
    }

    #[test]
    fn generates_display_and_into_response_for_unit_variant() {
        let input: TokenStream = quote! {
            enum MyError {
                #[error(status = 404, message = "not found")]
                NotFound,
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("Display"));
        assert!(output.contains("IntoResponse"));
        assert!(output.contains("not found"));
        assert!(output.contains("404u16"));
    }

    #[test]
    fn defaults_status_and_message() {
        let input: TokenStream = quote! {
            enum MyError {
                Bare,
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("500u16"));
        assert!(output.contains("Bare"));
    }

    #[test]
    fn handles_tuple_and_struct_variants() {
        let input: TokenStream = quote! {
            enum MyError {
                #[error(status = 409)]
                Tuple(String),
                #[error(message = "bad field")]
                Named { field: String },
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("Tuple (..)") || output.contains("Tuple(..)"));
        assert!(output.contains("Named"));
    }

    #[test]
    fn invalid_attribute_key_is_compile_error() {
        let input: TokenStream = quote! {
            enum MyError {
                #[error(bogus = 1)]
                Oops,
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("compile_error"));
    }
}
