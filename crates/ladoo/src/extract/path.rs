//! Typed path parameter extractor.
//!
//! `Path<T>` deserializes the route's captured `:param` segments into
//! typed Rust values. Supports single values, tuples, and structs.
//!
//! # Examples
//!
//! ```rust,ignore
//! use ladoo::extract::{Path, FromRequest};
//! use ladoo::request::Request;
//! use http::Method;
//!
//! let mut req = Request::test(Method::GET, "/users/42");
//! req.set_params(vec![("id".into(), "42".into())]);
//! let Path(id) = Path::<u64>::from_request(&mut req).unwrap();
//! assert_eq!(id, 42);
//! ```

use std::fmt;
use std::ops::Deref;

use serde::de::{
    self, DeserializeSeed, Deserializer, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use serde::forward_to_deserialize_any;

use super::FromRequest;
use crate::request::Request;
use crate::response::{IntoResponse, Response};

/// Extract typed path parameters from the request URL.
///
/// The inner type `T` is deserialized from the route's captured `:param`
/// segments. Supports single values, tuples, and structs.
///
/// # Examples
///
/// ```rust,ignore
/// use ladoo::extract::{Path, FromRequest};
/// use ladoo::request::Request;
/// use http::Method;
///
/// // Single parameter
/// let mut req = Request::test(Method::GET, "/users/42");
/// req.set_params(vec![("id".into(), "42".into())]);
/// let Path(id) = Path::<u64>::from_request(&mut req).unwrap();
/// assert_eq!(id, 42);
/// ```
#[derive(Debug)]
pub struct Path<T>(pub T);

impl<T> Deref for Path<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> FromRequest for Path<T>
where
    T: serde::de::DeserializeOwned,
{
    fn from_request(req: &mut Request) -> Result<Self, Response> {
        let params = req.params();
        T::deserialize(PathDeserializer { params })
            .map(Path)
            .map_err(|e| {
                if crate::error::is_dev_mode() {
                    (
                        http::StatusCode::BAD_REQUEST,
                        format!("Invalid path parameter: {e}"),
                    )
                        .into_response()
                } else {
                    (http::StatusCode::BAD_REQUEST, "Invalid path parameter").into_response()
                }
            })
    }
}

// ── Internal error type ──────────────────────────────────────────────

struct PathDeserializerError(String);

impl fmt::Display for PathDeserializerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for PathDeserializerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for PathDeserializerError {}

impl de::Error for PathDeserializerError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        PathDeserializerError(msg.to_string())
    }
}

// ── ValueDeserializer — parses a single &str into a typed value ──────

struct ValueDeserializer<'a> {
    value: &'a str,
}

macro_rules! forward_parsed_value {
    ($method:ident, $visit:ident, $ty:ty) => {
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            let v: $ty = self.value.parse().map_err(de::Error::custom)?;
            visitor.$visit(v)
        }
    };
}

impl<'de> Deserializer<'de> for ValueDeserializer<'de> {
    type Error = PathDeserializerError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_str(self.value)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_str(self.value)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_string(self.value.to_owned())
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let v: bool = self.value.parse().map_err(de::Error::custom)?;
        visitor.visit_bool(v)
    }

    forward_parsed_value!(deserialize_u8, visit_u8, u8);
    forward_parsed_value!(deserialize_u16, visit_u16, u16);
    forward_parsed_value!(deserialize_u32, visit_u32, u32);
    forward_parsed_value!(deserialize_u64, visit_u64, u64);
    forward_parsed_value!(deserialize_u128, visit_u128, u128);
    forward_parsed_value!(deserialize_i8, visit_i8, i8);
    forward_parsed_value!(deserialize_i16, visit_i16, i16);
    forward_parsed_value!(deserialize_i32, visit_i32, i32);
    forward_parsed_value!(deserialize_i64, visit_i64, i64);
    forward_parsed_value!(deserialize_i128, visit_i128, i128);
    forward_parsed_value!(deserialize_f32, visit_f32, f32);
    forward_parsed_value!(deserialize_f64, visit_f64, f64);
    forward_parsed_value!(deserialize_char, visit_char, char);

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_enum(ValueEnumAccess { value: self.value })
    }

    forward_to_deserialize_any! {
        bytes byte_buf unit unit_struct seq tuple tuple_struct
        map struct identifier ignored_any
    }
}

// ── Enum support for ValueDeserializer ───────────────────────────────

struct ValueEnumAccess<'a> {
    value: &'a str,
}

impl<'de> EnumAccess<'de> for ValueEnumAccess<'de> {
    type Error = PathDeserializerError;
    type Variant = UnitVariantAccess;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant), Self::Error>
    where
        V: DeserializeSeed<'de>,
    {
        let variant = seed.deserialize(self.value.into_deserializer())?;
        Ok((variant, UnitVariantAccess))
    }
}

struct UnitVariantAccess;

impl<'de> VariantAccess<'de> for UnitVariantAccess {
    type Error = PathDeserializerError;

    fn unit_variant(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn newtype_variant_seed<T>(self, _seed: T) -> Result<T::Value, Self::Error>
    where
        T: DeserializeSeed<'de>,
    {
        Err(de::Error::custom("path params do not support newtype enum variants"))
    }

    fn tuple_variant<V>(self, _len: usize, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(de::Error::custom("path params do not support tuple enum variants"))
    }

    fn struct_variant<V>(
        self,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(de::Error::custom("path params do not support struct enum variants"))
    }
}

// ── PathDeserializer — top-level deserializer over &[(String, String)] ─

struct PathDeserializer<'a> {
    params: &'a [(String, String)],
}

impl<'de> Deserializer<'de> for PathDeserializer<'de> {
    type Error = PathDeserializerError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        // Default: if there's exactly one param, treat as single value.
        // Otherwise, treat as a map (struct).
        if self.params.len() == 1 {
            ValueDeserializer {
                value: &self.params[0].1,
            }
            .deserialize_any(visitor)
        } else {
            self.deserialize_map(visitor)
        }
    }

    // Single scalar: take first (and only) param value.
    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_str(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_string(visitor)
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_bool(visitor)
    }

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_u8(visitor)
    }

    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_u16(visitor)
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_u32(visitor)
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_u64(visitor)
    }

    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_u128(visitor)
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_i8(visitor)
    }

    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_i16(visitor)
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_i32(visitor)
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_i64(visitor)
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_i128(visitor)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_f32(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_f64(visitor)
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_char(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.one_value()?.deserialize_enum(name, variants, visitor)
    }

    // Tuple: positional, params[0] → first element, params[1] → second, etc.
    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        if self.params.len() != len {
            return Err(de::Error::custom(format!(
                "wrong number of parameters: expected {len}, got {}",
                self.params.len()
            )));
        }
        visitor.visit_seq(SeqDeserializer {
            params: self.params,
            index: 0,
        })
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_tuple(len, visitor)
    }

    // Struct: named fields matched to param names.
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_map(MapDeserializer {
            params: self.params,
            index: 0,
            value: None,
        })
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_map(MapDeserializer {
            params: self.params,
            index: 0,
            value: None,
        })
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(SeqDeserializer {
            params: self.params,
            index: 0,
        })
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    forward_to_deserialize_any! {
        bytes byte_buf identifier ignored_any
    }
}

impl<'a> PathDeserializer<'a> {
    fn one_value(&self) -> Result<ValueDeserializer<'a>, PathDeserializerError> {
        if self.params.len() != 1 {
            return Err(de::Error::custom(format!(
                "wrong number of parameters: expected 1, got {}",
                self.params.len()
            )));
        }
        Ok(ValueDeserializer {
            value: &self.params[0].1,
        })
    }
}

// ── SeqAccess — for tuples (positional) ──────────────────────────────

struct SeqDeserializer<'a> {
    params: &'a [(String, String)],
    index: usize,
}

impl<'de> SeqAccess<'de> for SeqDeserializer<'de> {
    type Error = PathDeserializerError;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: DeserializeSeed<'de>,
    {
        if self.index >= self.params.len() {
            return Ok(None);
        }
        let value = &self.params[self.index].1;
        self.index += 1;
        seed.deserialize(ValueDeserializer { value }).map(Some)
    }
}

// ── MapAccess — for structs (named fields) ───────────────────────────

struct MapDeserializer<'a> {
    params: &'a [(String, String)],
    index: usize,
    value: Option<&'a str>,
}

impl<'de> MapAccess<'de> for MapDeserializer<'de> {
    type Error = PathDeserializerError;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: DeserializeSeed<'de>,
    {
        if self.index >= self.params.len() {
            return Ok(None);
        }
        let (key, value) = &self.params[self.index];
        self.value = Some(value.as_str());
        self.index += 1;
        seed.deserialize(key.as_str().into_deserializer()).map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: DeserializeSeed<'de>,
    {
        let value = self.value.take().expect("next_value_seed called before next_key_seed");
        seed.deserialize(ValueDeserializer { value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;
    use serde::Deserialize;

    // ── Single value ─────────────────────────────────────────────────

    #[test]
    fn single_u64() {
        let mut req = Request::test(Method::GET, "/users/42");
        req.set_params(vec![("id".into(), "42".into())]);
        let Path(id) = Path::<u64>::from_request(&mut req).unwrap();
        assert_eq!(id, 42);
    }

    #[test]
    fn single_string() {
        let mut req = Request::test(Method::GET, "/users/alice");
        req.set_params(vec![("name".into(), "alice".into())]);
        let Path(name) = Path::<String>::from_request(&mut req).unwrap();
        assert_eq!(name, "alice");
    }

    #[test]
    fn single_i32() {
        let mut req = Request::test(Method::GET, "/offset/-5");
        req.set_params(vec![("offset".into(), "-5".into())]);
        let Path(offset) = Path::<i32>::from_request(&mut req).unwrap();
        assert_eq!(offset, -5);
    }

    #[test]
    fn single_f64() {
        let mut req = Request::test(Method::GET, "/price/9.99");
        req.set_params(vec![("price".into(), "9.99".into())]);
        let Path(price) = Path::<f64>::from_request(&mut req).unwrap();
        assert!((price - 9.99).abs() < f64::EPSILON);
    }

    #[test]
    fn single_bool() {
        let mut req = Request::test(Method::GET, "/flag/true");
        req.set_params(vec![("flag".into(), "true".into())]);
        let Path(flag) = Path::<bool>::from_request(&mut req).unwrap();
        assert!(flag);
    }

    // ── Tuple ────────────────────────────────────────────────────────

    #[test]
    fn tuple_two_params() {
        let mut req = Request::test(Method::GET, "/orgs/acme/repos/42");
        req.set_params(vec![
            ("org".into(), "acme".into()),
            ("id".into(), "42".into()),
        ]);
        let Path((org, id)) = Path::<(String, u64)>::from_request(&mut req).unwrap();
        assert_eq!(org, "acme");
        assert_eq!(id, 42);
    }

    #[test]
    fn tuple_three_params() {
        let mut req = Request::test(Method::GET, "/a/b/c");
        req.set_params(vec![
            ("a".into(), "x".into()),
            ("b".into(), "y".into()),
            ("c".into(), "z".into()),
        ]);
        let Path((a, b, c)) = Path::<(String, String, String)>::from_request(&mut req).unwrap();
        assert_eq!(a, "x");
        assert_eq!(b, "y");
        assert_eq!(c, "z");
    }

    // ── Struct ───────────────────────────────────────────────────────

    #[test]
    fn named_struct() {
        #[derive(Deserialize)]
        struct ItemParams {
            category: String,
            id: u64,
        }

        let mut req = Request::test(Method::GET, "/items/books/42");
        req.set_params(vec![
            ("category".into(), "books".into()),
            ("id".into(), "42".into()),
        ]);
        let Path(params) = Path::<ItemParams>::from_request(&mut req).unwrap();
        assert_eq!(params.category, "books");
        assert_eq!(params.id, 42);
    }

    // ── Error cases ──────────────────────────────────────────────────

    #[test]
    fn type_mismatch_returns_400() {
        let mut req = Request::test(Method::GET, "/users/abc");
        req.set_params(vec![("id".into(), "abc".into())]);
        let result = Path::<u64>::from_request(&mut req);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn tuple_count_mismatch_returns_400() {
        let mut req = Request::test(Method::GET, "/orgs/acme");
        req.set_params(vec![("org".into(), "acme".into())]);
        let result = Path::<(String, u64)>::from_request(&mut req);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn zero_params_returns_400() {
        let mut req = Request::test(Method::GET, "/users");
        // No params set — empty vec by default.
        let result = Path::<u64>::from_request(&mut req);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status(), http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn error_hides_details_in_prod_mode() {
        let _guard = crate::error::tests::lock_env();
        std::env::remove_var("LADOO_ENV");
        std::env::remove_var("APP_ENV");

        let mut req = Request::test(Method::GET, "/users/abc");
        req.set_params(vec![("id".into(), "abc".into())]);
        let err = Path::<u64>::from_request(&mut req).unwrap_err();
        let body = String::from_utf8_lossy(err.body_bytes()).to_string();
        assert_eq!(body, "Invalid path parameter");
    }

    #[test]
    fn error_shows_details_in_dev_mode() {
        let _guard = crate::error::tests::lock_env();
        std::env::set_var("LADOO_ENV", "development");

        let mut req = Request::test(Method::GET, "/users/abc");
        req.set_params(vec![("id".into(), "abc".into())]);
        let err = Path::<u64>::from_request(&mut req).unwrap_err();
        let body = String::from_utf8_lossy(err.body_bytes()).to_string();
        assert!(body.starts_with("Invalid path parameter: "));
        assert!(body.contains("invalid digit"));

        std::env::remove_var("LADOO_ENV");
    }

    // ── Deref ────────────────────────────────────────────────────────

    #[test]
    fn deref_accesses_inner() {
        let mut req = Request::test(Method::GET, "/users/42");
        req.set_params(vec![("id".into(), "42".into())]);
        let path = Path::<u64>::from_request(&mut req).unwrap();
        let id: &u64 = &path;
        assert_eq!(*id, 42);
    }

    // ── Enum variant ─────────────────────────────────────────────────

    #[test]
    fn enum_variant() {
        #[derive(Debug, Deserialize, PartialEq)]
        #[serde(rename_all = "lowercase")]
        enum Color {
            Red,
            Blue,
            Green,
        }

        let mut req = Request::test(Method::GET, "/colors/blue");
        req.set_params(vec![("color".into(), "blue".into())]);
        let Path(color) = Path::<Color>::from_request(&mut req).unwrap();
        assert_eq!(color, Color::Blue);
    }
}
