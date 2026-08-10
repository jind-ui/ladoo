//! Implementation of `#[derive(Config)]`.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse2, Data, DeriveInput, Fields, Ident, Lit, LitStr, Token, Type,
};

enum ConfigAttr {
    Default(Lit),
    Env(String),
}

impl Parse for ConfigAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        if ident == "default" {
            let lit: Lit = input.parse()?;
            Ok(ConfigAttr::Default(lit))
        } else if ident == "env" {
            let lit: LitStr = input.parse()?;
            Ok(ConfigAttr::Env(lit.value()))
        } else {
            Err(syn::Error::new(
                ident.span(),
                "expected `default` or `env`",
            ))
        }
    }
}

struct FieldInfo {
    name: Ident,
    ty: Type,
    default: Option<Lit>,
    env_var: Option<String>,
    is_option: bool,
    inner_type_str: String,
}

fn is_option_type(ty: &Type) -> Option<&Type> {
    if let Type::Path(type_path) = ty {
        let segment = type_path.path.segments.last()?;
        if segment.ident == "Option" {
            if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                    return Some(inner);
                }
            }
        }
    }
    None
}

fn type_name_string(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}

fn parse_field(field: &syn::Field) -> syn::Result<FieldInfo> {
    let name = field.ident.clone().ok_or_else(|| {
        syn::Error::new(Span::call_site(), "#[derive(Config)] requires named fields")
    })?;

    let mut default = None;
    let mut env_var = None;

    for attr in &field.attrs {
        if !attr.path().is_ident("config") {
            continue;
        }
        let entries = attr.parse_args_with(
            syn::punctuated::Punctuated::<ConfigAttr, Token![,]>::parse_terminated,
        )?;
        for entry in entries {
            match entry {
                ConfigAttr::Default(lit) => default = Some(lit),
                ConfigAttr::Env(var) => env_var = Some(var),
            }
        }
    }

    let (is_option, inner_type_str) = if let Some(inner) = is_option_type(&field.ty) {
        (true, type_name_string(inner))
    } else {
        (false, type_name_string(&field.ty))
    };

    Ok(FieldInfo {
        name,
        ty: field.ty.clone(),
        default,
        env_var,
        is_option,
        inner_type_str,
    })
}

pub fn derive(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match parse2(input) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    let struct_ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return syn::Error::new(
                    Span::call_site(),
                    "#[derive(Config)] requires a struct with named fields",
                )
                .to_compile_error();
            }
        },
        _ => {
            return syn::Error::new(
                Span::call_site(),
                "#[derive(Config)] can only be used on structs",
            )
            .to_compile_error();
        }
    };

    let mut field_loaders = Vec::new();
    let mut field_names = Vec::new();

    for field in fields {
        let info = match parse_field(field) {
            Ok(info) => info,
            Err(err) => return err.to_compile_error(),
        };

        let name = &info.name;
        let ty = &info.ty;
        let type_str = &info.inner_type_str;
        let field_name_str = name.to_string();

        field_names.push(name.clone());

        let default_expr = info.default.as_ref().map(|lit| {
            if matches!(lit, Lit::Str(_)) {
                quote! { ::std::string::String::from(#lit) }
            } else {
                quote! { #lit }
            }
        });

        let toml_branch = if info.is_option {
            quote! {
                ::ladoo::config::parse_toml_value(&__toml_table, #field_name_str, #type_str)?
                    .or(::std::option::Option::None)
            }
        } else if let Some(ref default_tokens) = default_expr {
            quote! {
                ::ladoo::config::parse_toml_value(&__toml_table, #field_name_str, #type_str)?
                    .unwrap_or(#default_tokens)
            }
        } else {
            quote! {
                ::ladoo::config::parse_toml_value(&__toml_table, #field_name_str, #type_str)?
                    .ok_or_else(|| ::ladoo::config::ConfigError::MissingField {
                        field: #field_name_str,
                        expected_type: #type_str,
                    })?
            }
        };

        let loader = if let Some(ref env_var_name) = info.env_var {
            if info.is_option {
                quote! {
                    let #name: #ty = if let Ok(__val) = ::std::env::var(#env_var_name) {
                        Some(__val.parse().map_err(|_| ::ladoo::config::ConfigError::EnvVarParse {
                            var: ::std::string::String::from(#env_var_name),
                            value: __val,
                            expected_type: #type_str,
                        })?)
                    } else {
                        #toml_branch
                    };
                }
            } else if let Some(ref default_tokens) = default_expr {
                quote! {
                    let #name: #ty = if let Ok(__val) = ::std::env::var(#env_var_name) {
                        __val.parse().map_err(|_| ::ladoo::config::ConfigError::EnvVarParse {
                            var: ::std::string::String::from(#env_var_name),
                            value: __val,
                            expected_type: #type_str,
                        })?
                    } else {
                        ::ladoo::config::parse_toml_value(&__toml_table, #field_name_str, #type_str)?
                            .unwrap_or(#default_tokens)
                    };
                }
            } else {
                quote! {
                    let #name: #ty = if let Ok(__val) = ::std::env::var(#env_var_name) {
                        __val.parse().map_err(|_| ::ladoo::config::ConfigError::EnvVarParse {
                            var: ::std::string::String::from(#env_var_name),
                            value: __val,
                            expected_type: #type_str,
                        })?
                    } else {
                        ::ladoo::config::parse_toml_value(&__toml_table, #field_name_str, #type_str)?
                            .ok_or_else(|| ::ladoo::config::ConfigError::MissingField {
                                field: #field_name_str,
                                expected_type: #type_str,
                            })?
                    };
                }
            }
        } else {
            quote! {
                let #name: #ty = #toml_branch;
            }
        };

        field_loaders.push(loader);
    }

    quote! {
        impl #impl_generics ::ladoo::config::Config for #struct_ident #ty_generics #where_clause {
            fn load() -> ::std::result::Result<Self, ::ladoo::config::ConfigError> {
                let __toml_table = ::ladoo::config::load_toml_table()?;
                #(#field_loaders)*
                Ok(Self {
                    #(#field_names),*
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_enums() {
        let input: TokenStream = quote! {
            enum NotAStruct { A, B }
        };
        let output = derive(input).to_string();
        assert!(output.contains("compile_error"));
    }

    #[test]
    fn generates_config_impl_with_default() {
        let input: TokenStream = quote! {
            struct AppConfig {
                #[config(default = 3000)]
                port: u16,
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("Config"));
        assert!(output.contains("load"));
        assert!(output.contains("3000"));
        assert!(output.contains("parse_toml_value"));
    }

    #[test]
    fn generates_config_impl_with_env() {
        let input: TokenStream = quote! {
            struct AppConfig {
                #[config(env = "DATABASE_URL")]
                database_url: String,
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("DATABASE_URL"));
        assert!(output.contains("env :: var"));
        assert!(output.contains("MissingField"));
    }

    #[test]
    fn generates_config_impl_with_both() {
        let input: TokenStream = quote! {
            struct AppConfig {
                #[config(env = "PORT", default = 8080)]
                port: u16,
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("PORT"));
        assert!(output.contains("8080"));
    }

    #[test]
    fn generates_config_impl_required_field() {
        let input: TokenStream = quote! {
            struct AppConfig {
                host: String,
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("MissingField"));
        assert!(output.contains("host"));
    }

    #[test]
    fn handles_option_field() {
        let input: TokenStream = quote! {
            struct AppConfig {
                pool_size: Option<u32>,
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("None"));
        assert!(!output.contains("MissingField"));
    }

    #[test]
    fn handles_string_default() {
        let input: TokenStream = quote! {
            struct AppConfig {
                #[config(default = "0.0.0.0")]
                host: String,
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("0.0.0.0"));
        assert!(output.contains("String :: from"));
    }

    #[test]
    fn invalid_attribute_key_is_compile_error() {
        let input: TokenStream = quote! {
            struct AppConfig {
                #[config(bogus = 1)]
                port: u16,
            }
        };
        let output = derive(input).to_string();
        assert!(output.contains("compile_error"));
    }
}
