//! Implementation of `#[derive(Job)]`.
//!
//! Generates `Job` trait impl: `name()` from struct name (snake_case),
//! `config()` from `#[job(...)]` attributes, `handle()` delegates to
//! an inherent method of the same name.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse2, DeriveInput, Ident, LitInt, LitStr, Token,
};

enum JobField {
    Retries(u32),
    Timeout(u64),
    Backoff(String),
}

impl Parse for JobField {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        if ident == "retries" {
            let lit: LitInt = input.parse()?;
            Ok(JobField::Retries(lit.base10_parse()?))
        } else if ident == "timeout" {
            let lit: LitStr = input.parse()?;
            let secs = parse_duration(&lit.value()).map_err(|e| syn::Error::new(lit.span(), e))?;
            Ok(JobField::Timeout(secs))
        } else if ident == "backoff" {
            let lit: LitStr = input.parse()?;
            let val = lit.value();
            if val != "fixed" && val != "exponential" {
                return Err(syn::Error::new(
                    lit.span(),
                    "expected \"fixed\" or \"exponential\"",
                ));
            }
            Ok(JobField::Backoff(val))
        } else {
            Err(syn::Error::new(
                ident.span(),
                "expected `retries`, `timeout`, or `backoff`",
            ))
        }
    }
}

fn parse_duration(s: &str) -> Result<u64, String> {
    if s.is_empty() {
        return Err("empty duration string".to_string());
    }
    let (digits, unit) = if let Some(d) = s.strip_suffix('s') {
        (d, 1u64)
    } else if let Some(d) = s.strip_suffix('m') {
        (d, 60u64)
    } else if let Some(d) = s.strip_suffix('h') {
        (d, 3600u64)
    } else {
        return Err(format!(
            "invalid duration \"{s}\" — expected format like \"30s\", \"5m\", or \"1h\""
        ));
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("invalid number in duration \"{s}\""))?;
    Ok(n * unit)
}

fn to_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_lowercase().next().unwrap());
        } else {
            result.push(ch);
        }
    }
    result
}

pub fn derive(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match parse2(input) {
        Ok(input) => input,
        Err(err) => return err.to_compile_error(),
    };

    let struct_name = &input.ident;
    let snake_name = to_snake_case(&struct_name.to_string());

    let mut retries: u32 = 0;
    let mut timeout_secs: u64 = 30;
    let mut backoff_strategy = "exponential".to_string();

    for attr in &input.attrs {
        if !attr.path().is_ident("job") {
            continue;
        }
        let fields = match attr
            .parse_args_with(syn::punctuated::Punctuated::<JobField, Token![,]>::parse_terminated)
        {
            Ok(f) => f,
            Err(err) => return err.to_compile_error(),
        };
        for field in fields {
            match field {
                JobField::Retries(r) => retries = r,
                JobField::Timeout(t) => timeout_secs = t,
                JobField::Backoff(b) => backoff_strategy = b,
            }
        }
    }

    let backoff_tokens = if backoff_strategy == "fixed" {
        quote! { ladoo::job::BackoffStrategy::Fixed(::std::time::Duration::from_secs(1)) }
    } else {
        quote! { ladoo::job::BackoffStrategy::exponential_default() }
    };

    quote! {
        impl ladoo::job::Job for #struct_name {
            fn name(&self) -> &'static str {
                #snake_name
            }

            fn config(&self) -> ladoo::job::JobConfig {
                ladoo::job::JobConfig {
                    max_retries: #retries,
                    timeout: ::std::time::Duration::from_secs(#timeout_secs),
                    backoff: #backoff_tokens,
                }
            }

            fn handle(&self, ctx: &ladoo::job::JobContext) -> impl ::std::future::Future<Output = Result<(), ladoo::job::JobError>> + Send {
                self.handle(ctx)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_simple() {
        assert_eq!(to_snake_case("SendWelcomeEmail"), "send_welcome_email");
    }

    #[test]
    fn snake_case_single_word() {
        assert_eq!(to_snake_case("Cleanup"), "cleanup");
    }

    #[test]
    fn snake_case_consecutive_caps() {
        assert_eq!(to_snake_case("HTTPSRedirect"), "h_t_t_p_s_redirect");
    }

    #[test]
    fn parse_duration_seconds() {
        assert_eq!(parse_duration("30s").unwrap(), 30);
    }

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(parse_duration("5m").unwrap(), 300);
    }

    #[test]
    fn parse_duration_hours() {
        assert_eq!(parse_duration("1h").unwrap(), 3600);
    }

    #[test]
    fn parse_duration_invalid_unit() {
        assert!(parse_duration("5d").is_err());
    }

    #[test]
    fn parse_duration_empty() {
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn parse_duration_no_number() {
        assert!(parse_duration("s").is_err());
    }
}
