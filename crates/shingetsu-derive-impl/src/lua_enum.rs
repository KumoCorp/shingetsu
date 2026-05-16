//! `derive(FromLua)`, `derive(IntoLua)`, and `derive(LuaTyped)` for enums
//! with newtype variants.
//!
//! Each variant must be a single-field tuple variant (newtype).  The
//! generated `FromLua` tries each variant's inner `FromLua` in an
//! order determined by discriminant-set analysis — narrower types are
//! tried first so that e.g. `i64` is attempted before `f64`.
//!
//! The standalone `derive(LuaTyped)` produces a `LuaType::Union` of
//! the inner types, which flows into `ParamSpec.lua_type` for
//! type-checking tooling.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{DeriveInput, Fields, LitStr, Type};

// ---------------------------------------------------------------------------
// Enum-level tagging configuration
// ---------------------------------------------------------------------------

/// How variants are distinguished in the lua representation.
///
/// - **Untagged** (default): each variant's inner type is tried in
///   discriminant-priority order.  Matches the historical behavior.
/// - **Internal**: `{ <tag> = "Variant", ...inner_fields }`.  The inner
///   type's `IntoLua` must produce a Table.
/// - **Adjacent**: `{ <tag> = "Variant", <content> = inner_value }`.
///   Inner type can be anything.
pub(crate) enum Tagging {
    Untagged,
    Internal { tag: String },
    Adjacent { tag: String, content: String },
}

pub(crate) fn parse_enum_opts(attrs: &[syn::Attribute]) -> syn::Result<Tagging> {
    let mut tag: Option<String> = None;
    let mut content: Option<String> = None;
    let mut explicit_untagged = false;
    let mut span = proc_macro2::Span::call_site();
    for attr in attrs {
        if !attr.path().is_ident("lua") {
            continue;
        }
        span = attr.path().span();
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("tag") {
                let val: LitStr = meta.value()?.parse()?;
                tag = Some(val.value());
                Ok(())
            } else if meta.path.is_ident("content") {
                let val: LitStr = meta.value()?.parse()?;
                content = Some(val.value());
                Ok(())
            } else if meta.path.is_ident("untagged") {
                explicit_untagged = true;
                Ok(())
            } else {
                Err(meta.error("unknown lua enum option; expected `tag`, `content`, or `untagged`"))
            }
        })?;
    }
    match (tag, content, explicit_untagged) {
        (None, None, _) => Ok(Tagging::Untagged),
        (Some(t), None, false) => Ok(Tagging::Internal { tag: t }),
        (Some(t), Some(c), false) => Ok(Tagging::Adjacent { tag: t, content: c }),
        (None, Some(_), _) => Err(syn::Error::new(span, "`content` requires `tag` to be set")),
        (Some(_), _, true) => Err(syn::Error::new(
            span,
            "`tag` is incompatible with `untagged`",
        )),
    }
}

/// If every variant of `data` is a unit (data-less) variant, return
/// the `(ident, lua_name)` pairs (serde-default repr: the variant
/// name, or `#[lua(rename = "...")]`).  Returns `None` when any
/// variant carries data (the newtype-tagging path handles those).
pub(crate) fn unit_string_variants(
    data: &syn::DataEnum,
) -> Option<syn::Result<Vec<(syn::Ident, String)>>> {
    if data.variants.is_empty()
        || !data
            .variants
            .iter()
            .all(|v| matches!(v.fields, Fields::Unit))
    {
        return None;
    }
    let mut out = Vec::new();
    for v in &data.variants {
        match parse_variant_lua_name(v) {
            Ok(n) => out.push((v.ident.clone(), n)),
            Err(e) => return Some(Err(e)),
        }
    }
    Some(Ok(out))
}

pub(crate) fn parse_variant_lua_name(variant: &syn::Variant) -> syn::Result<String> {
    let mut name = variant.ident.to_string();
    for attr in &variant.attrs {
        if !attr.path().is_ident("lua") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let val: LitStr = meta.value()?.parse()?;
                name = val.value();
                Ok(())
            } else {
                Err(meta.error("unknown lua variant option; expected `rename`"))
            }
        })?;
    }
    Ok(name)
}

// ---------------------------------------------------------------------------
// Discriminant set analysis
// ---------------------------------------------------------------------------

/// The set of `Value` discriminants a Rust type can accept via `FromLua`.
///
/// Represented as a bitmask for easy subset/overlap checks.
#[derive(Clone, Copy, PartialEq, Eq)]
struct DiscriminantSet(u8);

#[allow(dead_code)]
impl DiscriminantSet {
    const BOOLEAN: u8 = 1 << 0;
    const INTEGER: u8 = 1 << 1;
    const FLOAT: u8 = 1 << 2;
    const STRING: u8 = 1 << 3;
    const TABLE: u8 = 1 << 4;
    const FUNCTION: u8 = 1 << 5;
    const USERDATA: u8 = 1 << 6;
    const ALL: u8 = 0x7F;

    fn size(self) -> u32 {
        self.0.count_ones()
    }

    fn overlaps(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    fn is_subset_of(self, other: Self) -> bool {
        self.0 & other.0 == self.0
    }
}

/// Map a Rust type to the set of `Value` discriminants its `FromLua`
/// impl can accept.  Returns `Err` for types that are not allowed as
/// enum variant inner types (e.g. `Option<T>`).
fn discriminant_set(ty: &Type) -> Result<DiscriminantSet, &'static str> {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            let name = seg.ident.to_string();
            return match name.as_str() {
                "bool" => Ok(DiscriminantSet(DiscriminantSet::BOOLEAN)),
                "i64" | "i32" | "u32" | "usize" => Ok(DiscriminantSet(
                    DiscriminantSet::INTEGER | DiscriminantSet::FLOAT,
                )),
                "f64" | "f32" => Ok(DiscriminantSet(
                    DiscriminantSet::INTEGER | DiscriminantSet::FLOAT,
                )),
                "Bytes" | "String" => Ok(DiscriminantSet(DiscriminantSet::STRING)),
                "Table" | "Vec" | "HashMap" | "BTreeMap" => {
                    Ok(DiscriminantSet(DiscriminantSet::TABLE))
                }
                "Function" => Ok(DiscriminantSet(DiscriminantSet::FUNCTION)),
                "Arc" | "Ud" | "UserDataRef" => {
                    Ok(DiscriminantSet(DiscriminantSet::USERDATA))
                }
                "Value" => Ok(DiscriminantSet(DiscriminantSet::ALL)),
                // Option<T> accepts nil plus T — ambiguous discriminant
                // set that changes based on T.  Not supported.
                "Option" => Err("Option<T> is not supported as an enum variant inner type; \
                     use the concrete type instead"),
                // Unknown types: assume table-backed (struct with derive(FromLua)).
                _ => Ok(DiscriminantSet(DiscriminantSet::TABLE)),
            };
        }
    }
    // Fallback for non-path types (references, etc.) — treat as table.
    Ok(DiscriminantSet(DiscriminantSet::TABLE))
}

// ---------------------------------------------------------------------------
// Variant info
// ---------------------------------------------------------------------------

pub(crate) struct VariantInfo<'a> {
    pub(crate) ident: &'a syn::Ident,
    /// Lua-facing name (variant ident or `#[lua(rename = ...)]`).
    pub(crate) lua_name: String,
    pub(crate) ty: &'a Type,
    discs: DiscriminantSet,
}

impl VariantInfo<'_> {
    /// A runtime guard expression (`bool`) that is true only when
    /// `value_expr` is one of the Lua kinds this variant's
    /// discriminant set accepts.  Used on the mlua side to enforce
    /// the strict (non-coercive) discriminant model: without it,
    /// mlua's coercive scalar `FromLua` would let e.g. a `String`
    /// variant swallow a number when tried first.
    pub(crate) fn mlua_kind_guard(&self, value_expr: &TokenStream) -> TokenStream {
        let d = self.discs.0;
        // The guard exists only to stop mlua's *coercive* scalar
        // `FromLua` from hijacking a sibling variant (e.g. a number
        // coerced into a `String`, or a numeric string into an
        // integer).  Only restrict when the variant's accepted set
        // is purely coercion-prone scalars; for table/userdata/
        // function or unknown (e.g. a unit-string `LuaRepr` enum,
        // which `discriminant_set` conservatively models as TABLE)
        // stay permissive and let the inner `FromLua` decide.
        const SCALAR: u8 = DiscriminantSet::BOOLEAN
            | DiscriminantSet::INTEGER
            | DiscriminantSet::FLOAT
            | DiscriminantSet::STRING;
        if d == 0 || d == DiscriminantSet::ALL || (d & !SCALAR) != 0 {
            return quote! { true };
        }
        let mut pats: Vec<TokenStream> = Vec::new();
        if d & DiscriminantSet::BOOLEAN != 0 {
            pats.push(quote! { ::mlua::Value::Boolean(_) });
        }
        if d & DiscriminantSet::INTEGER != 0 {
            pats.push(quote! { ::mlua::Value::Integer(_) });
        }
        if d & DiscriminantSet::FLOAT != 0 {
            pats.push(quote! { ::mlua::Value::Number(_) });
        }
        if d & DiscriminantSet::STRING != 0 {
            pats.push(quote! { ::mlua::Value::String(_) });
        }
        if d & DiscriminantSet::TABLE != 0 {
            pats.push(quote! { ::mlua::Value::Table(_) });
        }
        if d & DiscriminantSet::FUNCTION != 0 {
            pats.push(quote! { ::mlua::Value::Function(_) });
        }
        if d & DiscriminantSet::USERDATA != 0 {
            pats.push(quote! { ::mlua::Value::UserData(_) });
        }
        quote! { ::std::matches!(#value_expr, #(#pats)|*) }
    }
}

pub(crate) fn collect_variants(data: &syn::DataEnum) -> syn::Result<Vec<VariantInfo<'_>>> {
    let mut out = Vec::new();
    for variant in &data.variants {
        match &variant.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let field = fields.unnamed.first().expect("checked len == 1");
                let discs = discriminant_set(&field.ty)
                    .map_err(|msg| syn::Error::new_spanned(&field.ty, msg))?;
                let lua_name = parse_variant_lua_name(variant)?;
                out.push(VariantInfo {
                    ident: &variant.ident,
                    lua_name,
                    ty: &field.ty,
                    discs,
                });
            }
            Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    &variant.ident,
                    "FromLua/IntoLua derive on enums only supports newtype variants \
                     (single unnamed field)",
                ));
            }
            Fields::Unit => {
                return Err(syn::Error::new_spanned(
                    &variant.ident,
                    "FromLua/IntoLua derive on enums does not support unit variants",
                ));
            }
            Fields::Named(_) => {
                return Err(syn::Error::new_spanned(
                    &variant.ident,
                    "FromLua/IntoLua derive on enums does not support struct variants; \
                     use newtype variants instead",
                ));
            }
        }
    }
    Ok(out)
}

/// Sort variants by discriminant set size (ascending) and validate that
/// no two variants have ambiguous overlap.
pub(crate) fn sort_and_validate(variants: &mut [VariantInfo<'_>]) -> syn::Result<()> {
    // Sort by set size (stable sort preserves declaration order for
    // equal-size disjoint sets).
    variants.sort_by_key(|v| v.discs.size());

    // Pairwise overlap check.
    for i in 0..variants.len() {
        for j in (i + 1)..variants.len() {
            let a = &variants[i];
            let b = &variants[j];
            if !a.discs.overlaps(b.discs) {
                // Disjoint — no conflict.
                continue;
            }
            if a.discs.is_subset_of(b.discs) {
                // a is narrower and sorted first — correct.
                continue;
            }
            // Overlapping but neither is a subset — ambiguous.
            return Err(syn::Error::new_spanned(
                b.ident,
                format!(
                    "ambiguous overlap between variants `{}` and `{}`; \
                     their accepted Lua types overlap but neither is a \
                     subset of the other",
                    a.ident, b.ident,
                ),
            ));
        }
    }

    // Check for identical discriminant sets.  Normally an error, but
    // multiple *userdata-only* variants are permitted: their `FromLua`
    // does a concrete typed downcast that fails for the wrong
    // userdata type, so they are runtime-distinguishable (unlike,
    // say, `i64` vs `f64`, which share the number representation).
    let userdata_only = DiscriminantSet(DiscriminantSet::USERDATA);
    for i in 0..variants.len() {
        for j in (i + 1)..variants.len() {
            if variants[i].discs == variants[j].discs {
                if variants[i].discs == userdata_only {
                    continue;
                }
                return Err(syn::Error::new_spanned(
                    variants[j].ident,
                    format!(
                        "variants `{}` and `{}` accept the same Lua types; \
                         this is ambiguous",
                        variants[i].ident, variants[j].ident,
                    ),
                ));
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// derive(FromLua)
// ---------------------------------------------------------------------------

pub fn derive_enum_from_lua(parsed: &DeriveInput, data: &syn::DataEnum) -> TokenStream {
    let name = &parsed.ident;
    if let Some(units) = unit_string_variants(data) {
        let units = match units {
            Ok(u) => u,
            Err(e) => return e.to_compile_error(),
        };
        let arms = units.iter().map(|(id, n)| {
            let nb = n.as_bytes().to_vec();
            quote! { &[ #(#nb),* ] => ::std::result::Result::Ok(#name::#id), }
        });
        let names: Vec<&str> = units.iter().map(|(_, n)| n.as_str()).collect();
        let expected = names.join("`, `");
        return quote! {
            impl ::shingetsu::FromLua for #name {
                fn from_lua(__value: ::shingetsu::Value) -> ::std::result::Result<Self, ::shingetsu::VmError> {
                    let __s = match &__value {
                        ::shingetsu::Value::String(__b) => __b.as_ref().to_vec(),
                        _ => return ::std::result::Result::Err(::shingetsu::VmError::HostError {
                            name: ::std::string::String::new(),
                            source: ::std::format!(
                                "expected one of `{}` for {}",
                                #expected, ::std::stringify!(#name)
                            ).into(),
                        }),
                    };
                    match __s.as_slice() {
                        #(#arms)*
                        __other => ::std::result::Result::Err(::shingetsu::VmError::HostError {
                            name: ::std::string::String::new(),
                            source: ::std::format!(
                                "unknown {} variant `{}`; expected one of `{}`",
                                ::std::stringify!(#name),
                                ::std::string::String::from_utf8_lossy(__other),
                                #expected
                            ).into(),
                        }),
                    }
                }
            }
        };
    }
    let tagging = match parse_enum_opts(&parsed.attrs) {
        Ok(t) => t,
        Err(e) => return e.to_compile_error(),
    };
    let mut variants = match collect_variants(data) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    if variants.is_empty() {
        return syn::Error::new_spanned(name, "FromLua derive requires at least one variant")
            .to_compile_error();
    }

    let result = match &tagging {
        Tagging::Untagged => from_lua_untagged(name, &mut variants),
        // Tagged variants don't need disjoint discriminant sets — the tag
        // value disambiguates.  Sort/validate is therefore skipped.
        Tagging::Internal { tag } => from_lua_internal(name, &variants, tag),
        Tagging::Adjacent { tag, content } => from_lua_adjacent(name, &variants, tag, content),
    };
    result.unwrap_or_else(|e: syn::Error| e.to_compile_error())
}

fn from_lua_untagged(
    name: &syn::Ident,
    variants: &mut Vec<VariantInfo<'_>>,
) -> syn::Result<TokenStream> {
    sort_and_validate(variants)?;

    // Build the `expected` string at runtime using LuaTyped so that
    // concrete userdata types show their Lua name (e.g. "file") rather
    // than the generic "userdata".
    let type_exprs: Vec<TokenStream> = variants
        .iter()
        .map(|v| {
            let ty = v.ty;
            quote! { <#ty as ::shingetsu::LuaTyped>::lua_type().simple_type_name() }
        })
        .collect();
    let expected_expr = quote! {
        [#(#type_exprs),*].join(" | ")
    };

    // Generate try-arms.  All but the last clone the value.
    let last_idx = variants.len() - 1;
    let try_arms: Vec<TokenStream> = variants
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let variant_ident = v.ident;
            let ty = v.ty;
            if i < last_idx {
                quote! {
                    if let ::std::result::Result::Ok(inner) =
                        <#ty as ::shingetsu::FromLua>::from_lua(__value.clone())
                    {
                        return ::std::result::Result::Ok(#name::#variant_ident(inner));
                    }
                }
            } else {
                quote! {
                    if let ::std::result::Result::Ok(inner) =
                        <#ty as ::shingetsu::FromLua>::from_lua(__value)
                    {
                        return ::std::result::Result::Ok(#name::#variant_ident(inner));
                    }
                }
            }
        })
        .collect();

    Ok(quote! {
        impl ::shingetsu::FromLua for #name {
            fn from_lua(__value: ::shingetsu::Value) -> ::std::result::Result<Self, ::shingetsu::VmError> {
                let __type_name = __value.type_name();
                #(#try_arms)*
                ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                    position: 0,
                    function: ::std::string::String::new(),
                    expected: #expected_expr,
                    got: __type_name.to_owned(),
                })
            }
        }
    })
}

fn from_lua_internal(
    name: &syn::Ident,
    variants: &[VariantInfo<'_>],
    tag: &str,
) -> syn::Result<TokenStream> {
    let arms: Vec<TokenStream> = variants
        .iter()
        .map(|v| {
            let variant_ident = v.ident;
            let ty = v.ty;
            let lua_name = &v.lua_name;
            quote! {
                #lua_name => {
                    let inner = <#ty as ::shingetsu::FromLua>::from_lua(
                        ::shingetsu::Value::Table(__table.clone())
                    )?;
                    ::std::result::Result::Ok(#name::#variant_ident(inner))
                }
            }
        })
        .collect();
    let known: Vec<&str> = variants.iter().map(|v| v.lua_name.as_str()).collect();
    let known_joined = known.join(" | ");

    Ok(quote! {
        impl ::shingetsu::FromLua for #name {
            fn from_lua(__value: ::shingetsu::Value) -> ::std::result::Result<Self, ::shingetsu::VmError> {
                let __table = match __value {
                    ::shingetsu::Value::Table(t) => t,
                    other => {
                        return ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                            position: 0,
                            function: ::std::string::String::new(),
                            expected: "table".to_owned(),
                            got: other.type_name().to_owned(),
                        });
                    }
                };
                let __tag_value = __table.raw_get(
                    &::shingetsu::Value::String(::shingetsu::Bytes::from(#tag))
                )?;
                let __tag: ::shingetsu::Bytes = match __tag_value {
                    ::shingetsu::Value::String(s) => s,
                    other => {
                        return ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                            position: 0,
                            function: ::std::string::String::new(),
                            expected: ::std::format!("string `{}` tag", #tag),
                            got: other.type_name().to_owned(),
                        });
                    }
                };
                let __tag_str: &str = match ::std::str::from_utf8(__tag.as_ref()) {
                    ::std::result::Result::Ok(s) => s,
                    ::std::result::Result::Err(_) => {
                        return ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                            position: 0,
                            function: ::std::string::String::new(),
                            expected: ::std::format!("one of: {}", #known_joined),
                            got: "non-utf8 tag".to_owned(),
                        });
                    }
                };
                match __tag_str {
                    #(#arms)*
                    other => ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                        position: 0,
                        function: ::std::string::String::new(),
                        expected: ::std::format!("one of: {}", #known_joined),
                        got: ::std::format!("unknown tag `{}`", other),
                    }),
                }
            }
        }
    })
}

fn from_lua_adjacent(
    name: &syn::Ident,
    variants: &[VariantInfo<'_>],
    tag: &str,
    content: &str,
) -> syn::Result<TokenStream> {
    let arms: Vec<TokenStream> = variants
        .iter()
        .map(|v| {
            let variant_ident = v.ident;
            let ty = v.ty;
            let lua_name = &v.lua_name;
            quote! {
                #lua_name => {
                    let __content = __table.raw_get(
                        &::shingetsu::Value::String(::shingetsu::Bytes::from(#content))
                    )?;
                    let inner = <#ty as ::shingetsu::FromLua>::from_lua(__content)?;
                    ::std::result::Result::Ok(#name::#variant_ident(inner))
                }
            }
        })
        .collect();
    let known: Vec<&str> = variants.iter().map(|v| v.lua_name.as_str()).collect();
    let known_joined = known.join(" | ");

    Ok(quote! {
        impl ::shingetsu::FromLua for #name {
            fn from_lua(__value: ::shingetsu::Value) -> ::std::result::Result<Self, ::shingetsu::VmError> {
                let __table = match __value {
                    ::shingetsu::Value::Table(t) => t,
                    other => {
                        return ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                            position: 0,
                            function: ::std::string::String::new(),
                            expected: "table".to_owned(),
                            got: other.type_name().to_owned(),
                        });
                    }
                };
                let __tag_value = __table.raw_get(
                    &::shingetsu::Value::String(::shingetsu::Bytes::from(#tag))
                )?;
                let __tag: ::shingetsu::Bytes = match __tag_value {
                    ::shingetsu::Value::String(s) => s,
                    other => {
                        return ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                            position: 0,
                            function: ::std::string::String::new(),
                            expected: ::std::format!("string `{}` tag", #tag),
                            got: other.type_name().to_owned(),
                        });
                    }
                };
                let __tag_str: &str = match ::std::str::from_utf8(__tag.as_ref()) {
                    ::std::result::Result::Ok(s) => s,
                    ::std::result::Result::Err(_) => {
                        return ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                            position: 0,
                            function: ::std::string::String::new(),
                            expected: ::std::format!("one of: {}", #known_joined),
                            got: "non-utf8 tag".to_owned(),
                        });
                    }
                };
                match __tag_str {
                    #(#arms)*
                    other => ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                        position: 0,
                        function: ::std::string::String::new(),
                        expected: ::std::format!("one of: {}", #known_joined),
                        got: ::std::format!("unknown tag `{}`", other),
                    }),
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// derive(LuaTyped) for enums
// ---------------------------------------------------------------------------

pub fn derive_enum_lua_typed(parsed: &DeriveInput, data: &syn::DataEnum) -> TokenStream {
    let name = &parsed.ident;
    if unit_string_variants(data).is_some() {
        return quote! {
            impl ::shingetsu::LuaTyped for #name {
                fn lua_type() -> ::shingetsu::LuaType {
                    ::shingetsu::LuaType::String
                }
            }
        };
    }
    let tagging = match parse_enum_opts(&parsed.attrs) {
        Ok(t) => t,
        Err(e) => return e.to_compile_error(),
    };
    let variants = match collect_variants(data) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    match tagging {
        Tagging::Untagged => {
            let type_exprs: Vec<TokenStream> = variants
                .iter()
                .map(|v| {
                    let ty = v.ty;
                    quote! { <#ty as ::shingetsu::LuaTyped>::lua_type() }
                })
                .collect();
            quote! {
                impl ::shingetsu::LuaTyped for #name {
                    fn lua_type() -> ::shingetsu::LuaType {
                        ::shingetsu::LuaType::Union(::std::vec![ #(#type_exprs),* ])
                    }
                }
            }
        }
        Tagging::Internal { tag } => {
            let tag_bytes = tag.as_bytes().to_vec();
            let variant_types: Vec<TokenStream> = variants
                .iter()
                .map(|v| {
                    let ty = v.ty;
                    let lua_name_bytes = v.lua_name.as_bytes().to_vec();
                    quote! {
                        {
                            let mut __vfields: ::std::vec::Vec<::shingetsu::TableField> =
                                ::std::vec::Vec::new();
                            __vfields.push(::shingetsu::TableField::new(
                                ::shingetsu::Bytes::from(&[ #(#tag_bytes),* ][..]),
                                ::shingetsu::LuaType::StringLiteral(
                                    ::shingetsu::Bytes::from(&[ #(#lua_name_bytes),* ][..])
                                ),
                            ));
                            // Splat inner table fields if the inner type is
                            // a Table.  Non-table inner types fall back to
                            // a tag-only signature.
                            if let ::shingetsu::LuaType::Table(__t) =
                                <#ty as ::shingetsu::LuaTyped>::lua_type()
                            {
                                for __field in __t.fields {
                                    __vfields.push(__field);
                                }
                            }
                            ::shingetsu::LuaType::Table(::std::boxed::Box::new(
                                ::shingetsu::TableLuaType {
                                    fields: __vfields,
                                    indexer: ::std::option::Option::None,
                                }
                            ))
                        }
                    }
                })
                .collect();
            quote! {
                impl ::shingetsu::LuaTyped for #name {
                    fn lua_type() -> ::shingetsu::LuaType {
                        ::shingetsu::LuaType::Union(::std::vec![ #(#variant_types),* ])
                    }
                }
            }
        }
        Tagging::Adjacent { tag, content } => {
            let tag_bytes = tag.as_bytes().to_vec();
            let content_bytes = content.as_bytes().to_vec();
            let variant_types: Vec<TokenStream> = variants
                .iter()
                .map(|v| {
                    let ty = v.ty;
                    let lua_name_bytes = v.lua_name.as_bytes().to_vec();
                    quote! {
                        ::shingetsu::LuaType::Table(::std::boxed::Box::new(
                            ::shingetsu::TableLuaType {
                                fields: ::std::vec![
                                    ::shingetsu::TableField::new(
                                        ::shingetsu::Bytes::from(&[ #(#tag_bytes),* ][..]),
                                        ::shingetsu::LuaType::StringLiteral(
                                            ::shingetsu::Bytes::from(
                                                &[ #(#lua_name_bytes),* ][..]
                                            )
                                        ),
                                    ),
                                    ::shingetsu::TableField::new(
                                        ::shingetsu::Bytes::from(
                                            &[ #(#content_bytes),* ][..]
                                        ),
                                        <#ty as ::shingetsu::LuaTyped>::lua_type(),
                                    ),
                                ],
                                indexer: ::std::option::Option::None,
                            }
                        ))
                    }
                })
                .collect();
            quote! {
                impl ::shingetsu::LuaTyped for #name {
                    fn lua_type() -> ::shingetsu::LuaType {
                        ::shingetsu::LuaType::Union(::std::vec![ #(#variant_types),* ])
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// derive(IntoLua)
// ---------------------------------------------------------------------------

/// Check if a type path ends with `Variadic`.
fn is_variadic(ty: &Type) -> bool {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            return seg.ident == "Variadic";
        }
    }
    false
}

pub fn derive_enum_into_lua(parsed: &DeriveInput, data: &syn::DataEnum) -> TokenStream {
    let name = &parsed.ident;
    if let Some(units) = unit_string_variants(data) {
        let units = match units {
            Ok(u) => u,
            Err(e) => return e.to_compile_error(),
        };
        let arms = units.iter().map(|(id, n)| {
            quote! { #name::#id => ::shingetsu::Value::string(#n), }
        });
        return quote! {
            impl ::shingetsu::IntoLua for #name {
                fn into_lua(self) -> ::shingetsu::Value {
                    match self {
                        #(#arms)*
                    }
                }
            }
        };
    }
    let tagging = match parse_enum_opts(&parsed.attrs) {
        Ok(t) => t,
        Err(e) => return e.to_compile_error(),
    };
    let variants = match collect_variants(data) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    if variants.is_empty() {
        return syn::Error::new_spanned(name, "IntoLua derive requires at least one variant")
            .to_compile_error();
    }

    let arms: Vec<TokenStream> = variants
        .iter()
        .map(|v| {
            let variant_ident = v.ident;
            let lua_name = &v.lua_name;
            match &tagging {
                Tagging::Untagged => quote! {
                    #name::#variant_ident(inner) => ::shingetsu::IntoLua::into_lua(inner),
                },
                Tagging::Internal { tag } => quote! {
                    #name::#variant_ident(inner) => {
                        // The `LuaTableShape` bound on the inner type
                        // (enforced by the static check below) guarantees
                        // a Table here.
                        let __inner = ::shingetsu::LuaTableShape::into_lua_table(inner);
                        __inner.raw_set(
                            ::shingetsu::Value::String(
                                ::shingetsu::Bytes::from(#tag)
                            ),
                            ::shingetsu::Value::String(
                                ::shingetsu::Bytes::from(#lua_name)
                            ),
                        ).expect("set tag");
                        ::shingetsu::Value::Table(__inner)
                    }
                },
                Tagging::Adjacent { tag, content } => quote! {
                    #name::#variant_ident(inner) => {
                        let __outer = ::shingetsu::Table::new();
                        __outer.raw_set(
                            ::shingetsu::Value::String(::shingetsu::Bytes::from(#tag)),
                            ::shingetsu::Value::String(::shingetsu::Bytes::from(#lua_name)),
                        ).expect("set tag");
                        __outer.raw_set(
                            ::shingetsu::Value::String(::shingetsu::Bytes::from(#content)),
                            ::shingetsu::IntoLua::into_lua(inner),
                        ).expect("set content");
                        ::shingetsu::Value::Table(__outer)
                    }
                },
            }
        })
        .collect();

    // Compile-time assertion: for internally-tagged enums, every variant's
    // inner type must implement `LuaTableShape`.  This converts what would
    // otherwise be a runtime panic into a compile error.
    let static_table_shape_assert = match &tagging {
        Tagging::Internal { .. } => {
            let asserts: Vec<TokenStream> = variants
                .iter()
                .map(|v| {
                    let ty = v.ty;
                    quote! {
                        const _: fn() = || {
                            fn __assert_lua_table_shape<T: ::shingetsu::LuaTableShape>() {}
                            __assert_lua_table_shape::<#ty>();
                        };
                    }
                })
                .collect();
            quote! { #(#asserts)* }
        }
        _ => quote! {},
    };

    // Emit `impl LuaTableShape` for tagging modes whose output is always
    // a `Value::Table`.  Untagged is omitted: variants may produce any
    // Lua type.
    let table_shape_impl = match &tagging {
        Tagging::Internal { .. } | Tagging::Adjacent { .. } => quote! {
            impl ::shingetsu::LuaTableShape for #name {}
        },
        Tagging::Untagged => quote! {},
    };

    quote! {
        #static_table_shape_assert

        impl ::shingetsu::IntoLua for #name {
            fn into_lua(self) -> ::shingetsu::Value {
                match self {
                    #(#arms)*
                }
            }
        }

        #table_shape_impl
    }
}

// ---------------------------------------------------------------------------
// derive(IntoLuaMulti) for enums
// ---------------------------------------------------------------------------

pub fn derive_enum_into_lua_multi(input: TokenStream) -> TokenStream {
    let parsed: DeriveInput = match syn::parse2(input) {
        Ok(p) => p,
        Err(e) => return e.to_compile_error(),
    };
    let name = &parsed.ident;
    let data = match &parsed.data {
        syn::Data::Enum(e) => e,
        _ => {
            return syn::Error::new_spanned(name, "IntoLuaMulti derive only supports enums")
                .to_compile_error();
        }
    };

    if data.variants.is_empty() {
        return syn::Error::new_spanned(name, "IntoLuaMulti derive requires at least one variant")
            .to_compile_error();
    }

    let arms: Vec<TokenStream> = data
        .variants
        .iter()
        .map(|variant| {
            let variant_ident = &variant.ident;
            match &variant.fields {
                Fields::Unit => {
                    // Unit variant → nil
                    quote! {
                        #name::#variant_ident => ::shingetsu::valuevec![::shingetsu::Value::Nil],
                    }
                }
                Fields::Unnamed(fields) => {
                    let field_count = fields.unnamed.len();
                    let bindings: Vec<syn::Ident> = (0..field_count)
                        .map(|i| quote::format_ident!("__f{}", i))
                        .collect();
                    let bind_pat = &bindings;

                    // Check if the last field is Variadic.
                    let last_is_variadic = fields
                        .unnamed
                        .last()
                        .map(|f| is_variadic(&f.ty))
                        .unwrap_or(false);

                    if last_is_variadic && field_count > 1 {
                        // Fields before the last are pushed via IntoLua;
                        // the last (Variadic) is extended.
                        let regular = &bindings[..field_count - 1];
                        let variadic = &bindings[field_count - 1];
                        let pushes: Vec<TokenStream> = regular
                            .iter()
                            .map(|b| {
                                quote! { __out.push(::shingetsu::IntoLua::into_lua(#b)); }
                            })
                            .collect();
                        quote! {
                            #name::#variant_ident( #(#bind_pat),* ) => {
                                let mut __out = ::shingetsu::ValueVec::new();
                                #(#pushes)*
                                __out.extend(#variadic.0);
                                __out
                            }
                        }
                    } else if last_is_variadic {
                        // Single Variadic field — just return its contents.
                        let variadic = &bindings[0];
                        quote! {
                            #name::#variant_ident( #variadic ) => #variadic.0,
                        }
                    } else {
                        // All fields are regular IntoLua.
                        let pushes: Vec<TokenStream> = bindings
                            .iter()
                            .map(|b| {
                                quote! { ::shingetsu::IntoLua::into_lua(#b) }
                            })
                            .collect();
                        quote! {
                            #name::#variant_ident( #(#bind_pat),* ) => {
                                ::shingetsu::valuevec![ #(#pushes),* ]
                            }
                        }
                    }
                }
                Fields::Named(_) => {
                    // Struct variants aren't useful for multi-return.
                    syn::Error::new_spanned(
                        variant_ident,
                        "IntoLuaMulti derive does not support struct variants",
                    )
                    .to_compile_error()
                }
            }
        })
        .collect();

    // Build LuaTypedMulti body: union of each variant's return shape.
    let variant_types: Vec<TokenStream> = data
        .variants
        .iter()
        .map(|variant| {
            match &variant.fields {
                Fields::Unit => {
                    // Unit variant → nil
                    quote! { ::shingetsu::LuaType::Nil }
                }
                Fields::Unnamed(fields) => {
                    let field_count = fields.unnamed.len();
                    let last_is_variadic = fields
                        .unnamed
                        .last()
                        .map(|f| is_variadic(&f.ty))
                        .unwrap_or(false);

                    let type_exprs: Vec<TokenStream> = fields
                        .unnamed
                        .iter()
                        .enumerate()
                        .map(|(i, f)| {
                            let ty = &f.ty;
                            if i == field_count - 1 && last_is_variadic {
                                quote! { <#ty as ::shingetsu::LuaTyped>::lua_type() }
                            } else {
                                quote! { <#ty as ::shingetsu::LuaTyped>::lua_type() }
                            }
                        })
                        .collect();

                    if type_exprs.len() == 1 {
                        // Single field: use its type directly.
                        let t = &type_exprs[0];
                        quote! { #t }
                    } else {
                        // Multiple fields: wrap in Tuple.
                        quote! {
                            ::shingetsu::LuaType::Tuple(
                                ::std::vec![ #(#type_exprs),* ]
                            )
                        }
                    }
                }
                Fields::Named(_) => {
                    // Already rejected in the arms generation above.
                    quote! { ::shingetsu::LuaType::Any }
                }
            }
        })
        .collect();

    let lua_typed_multi_body = if variant_types.len() == 1 {
        let t = &variant_types[0];
        quote! { ::std::vec![#t] }
    } else {
        quote! {
            ::std::vec![
                ::shingetsu::LuaType::Union(
                    ::std::vec![ #(#variant_types),* ]
                )
            ]
        }
    };

    quote! {
        impl ::shingetsu::IntoLuaMulti for #name {
            fn into_lua_multi(self) -> ::shingetsu::ValueVec {
                match self {
                    #(#arms)*
                }
            }
        }

        impl ::shingetsu::LuaTypedMulti for #name {
            fn lua_types() -> ::std::vec::Vec<::shingetsu::LuaType> {
                #lua_typed_multi_body
            }
        }
    }
}

// ---------------------------------------------------------------------------
// derive(FromLuaMulti) for enums
// ---------------------------------------------------------------------------

pub fn derive_enum_from_lua_multi(input: TokenStream) -> TokenStream {
    let parsed: DeriveInput = match syn::parse2(input) {
        Ok(p) => p,
        Err(e) => return e.to_compile_error(),
    };
    let name = &parsed.ident;
    let data = match &parsed.data {
        syn::Data::Enum(e) => e,
        _ => {
            return syn::Error::new_spanned(name, "FromLuaMulti derive only supports enums")
                .to_compile_error();
        }
    };

    if data.variants.is_empty() {
        return syn::Error::new_spanned(name, "FromLuaMulti derive requires at least one variant")
            .to_compile_error();
    }

    // Collect per-variant info.  `min_arity` is the number of
    // non-variadic fields (== arity for variants without a
    // trailing `Variadic`); arity-matching uses `__n >=
    // min_arity` for variadic variants and `__n == arity` for
    // strict ones.
    struct VariantInfo<'a> {
        ident: &'a syn::Ident,
        types: Vec<&'a syn::Type>,
        names: Vec<Option<String>>,
        arity: usize,
        min_arity: usize,
        is_named: bool,
        last_is_variadic: bool,
    }

    let mut variants: Vec<VariantInfo> = Vec::new();
    for v in &data.variants {
        let (types, names, is_named) = match &v.fields {
            Fields::Unnamed(fields) => {
                let types: Vec<&syn::Type> = fields.unnamed.iter().map(|f| &f.ty).collect();
                let names = fields.unnamed.iter().map(|_| None).collect();
                (types, names, false)
            }
            Fields::Unit => (Vec::new(), Vec::new(), false),
            Fields::Named(fields) => {
                let types: Vec<&syn::Type> = fields.named.iter().map(|f| &f.ty).collect();
                let names: Vec<Option<String>> = fields
                    .named
                    .iter()
                    .map(|f| f.ident.as_ref().map(|i| i.to_string()))
                    .collect();
                (types, names, true)
            }
        };
        let arity = types.len();
        let last_is_variadic = types.last().map(|t| is_variadic(t)).unwrap_or(false);
        let min_arity = if last_is_variadic { arity - 1 } else { arity };
        variants.push(VariantInfo {
            ident: &v.ident,
            types,
            names,
            arity,
            min_arity,
            is_named,
            last_is_variadic,
        });
    }
    // Sort by descending min_arity (longest required prefix first);
    // ties prefer non-variadic over variadic so a strict-arity match
    // wins over a variadic that happens to overlap.
    variants.sort_by(|a, b| {
        b.min_arity
            .cmp(&a.min_arity)
            .then((!a.last_is_variadic).cmp(&(!b.last_is_variadic)))
    });

    // Generate match arms: try each variant in priority order.
    // Each arm is wrapped in an immediately-invoked closure so a
    // FromLua failure on one variant falls through to the next
    // (with the last error preserved for the no-match diagnostic).
    let mut arms = Vec::<TokenStream>::new();
    for v in &variants {
        let variant_ident = v.ident;
        if v.arity == 0 {
            arms.push(quote! {
                if __n == 0 {
                    return Ok(#name::#variant_ident);
                }
            });
            continue;
        }
        let field_idents: Vec<syn::Ident> = (0..v.arity)
            .map(|i| quote::format_ident!("__f{}", i))
            .collect();

        let min_arity = v.min_arity;
        let arity_check = if v.last_is_variadic {
            quote! { __n >= #min_arity }
        } else {
            let arity = v.arity;
            quote! { __n == #arity }
        };

        let last_idx = v.arity - 1;
        let extraction_bindings: Vec<TokenStream> = v
            .types
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                let fid = &field_idents[i];
                if v.last_is_variadic && i == last_idx {
                    // Absorb trailing args into a Variadic.  The
                    // FromLua chain isn't applicable here; Variadic
                    // collects whatever is left as raw `Value`s.
                    quote! {
                        let #fid = <#ty as ::std::convert::From<
                            ::shingetsu::ValueVec,
                        >>::from(
                            __vals.iter().skip(#i).cloned().collect()
                        );
                    }
                } else {
                    let pos = i + 1;
                    quote! {
                        let __v = __vals.get(#i).cloned().unwrap_or(::shingetsu::Value::Nil);
                        let #fid = match <#ty as ::shingetsu::FromLua>::from_lua(__v) {
                            Ok(v) => v,
                            Err(_) => {
                                return Err(::shingetsu::VmError::BadArgument {
                                    position: #pos,
                                    function: ::std::string::String::new(),
                                    expected: <#ty as ::shingetsu::LuaTyped>::lua_type().to_string(),
                                    got: __vals.get(#i)
                                        .unwrap_or(&::shingetsu::Value::Nil)
                                        .type_name()
                                        .to_owned(),
                                });
                            }
                        };
                    }
                }
            })
            .collect();

        let construct = if v.is_named {
            let assignments: Vec<TokenStream> = v
                .names
                .iter()
                .zip(field_idents.iter())
                .map(|(name, fid)| {
                    let name_ident = syn::Ident::new(
                        name.as_ref().expect("named field has identifier"),
                        proc_macro2::Span::call_site(),
                    );
                    quote! { #name_ident: #fid }
                })
                .collect();
            quote! { #name::#variant_ident { #(#assignments),* } }
        } else {
            quote! { #name::#variant_ident( #(#field_idents),* ) }
        };

        arms.push(quote! {
            if #arity_check {
                let __r: ::std::result::Result<#name, ::shingetsu::VmError> =
                    (|| -> ::std::result::Result<#name, ::shingetsu::VmError> {
                        #(#extraction_bindings)*
                        Ok(#construct)
                    })();
                match __r {
                    Ok(__v) => return Ok(__v),
                    Err(__e) => __last_err = Some(__e),
                }
            }
        });
    }

    // Build LuaTypedMulti.  Two strategies:
    //
    // 1. *Leading-optional chain*: when sorted variants form a
    //    strict tail-subset chain (each subsequent variant is the
    //    previous one with one extra leading field stripped, e.g.
    //    `Named { name, func, args }` and `NoName { func, args }`)
    //    we render using the longest variant's names and types,
    //    wrapping the leading positions in `Optional` to convey
    //    the standard "first arg is optional" overload shape.
    //
    // 2. *Per-position union* (default): emit the union of types
    //    contributing to each position, wrapping positions beyond
    //    the shortest variant in `Optional` (except those that
    //    contain a `Variadic`, which already carries "zero or
    //    more" semantics).  This is the right shape for
    //    overloads like `table.insert(t, value)` /
    //    `table.insert(t, pos, value)` where the variants don't
    //    share a common tail.
    let max_arity = variants.iter().map(|v| v.arity).max().unwrap_or(0);
    let min_arity = variants.iter().map(|v| v.arity).min().unwrap_or(0);

    let leading_optional_count = if variants.len() >= 2 {
        let mut count = 0usize;
        let mut chain_holds = true;
        for pair in variants.windows(2) {
            let longer = &pair[0];
            let shorter = &pair[1];
            if longer.types.len() != shorter.types.len() + 1 {
                chain_holds = false;
                break;
            }
            let tail_match = longer
                .types
                .iter()
                .skip(1)
                .zip(shorter.types.iter())
                .all(|(a, b)| **a == **b);
            if !tail_match {
                chain_holds = false;
                break;
            }
            count += 1;
        }
        if chain_holds {
            count
        } else {
            0
        }
    } else {
        0
    };

    let mut pos_type_exprs = Vec::<TokenStream>::new();
    let mut pos_name_exprs = Vec::<TokenStream>::new();

    if leading_optional_count > 0 {
        // Strategy 1: longest variant's structure with the first
        // `leading_optional_count` positions wrapped in Optional.
        let longest = &variants[0];
        for (i, ty) in longest.types.iter().enumerate() {
            let type_expr = quote! { <#ty as ::shingetsu::LuaTyped>::lua_type() };
            let wrapped = if i < leading_optional_count && !is_variadic(ty) {
                quote! {
                    ::shingetsu::LuaType::Optional(::std::boxed::Box::new(#type_expr))
                }
            } else {
                type_expr
            };
            pos_type_exprs.push(wrapped);
            let name_expr = match longest.names.get(i).and_then(|n| n.as_ref()) {
                Some(n) => quote! { ::std::option::Option::Some(#n) },
                None => quote! { ::std::option::Option::None },
            };
            pos_name_exprs.push(name_expr);
        }
    } else {
        // Strategy 2: per-position union.
        for i in 0..max_arity {
            let mut seen_types = Vec::<&syn::Type>::new();
            let mut has_variadic = false;
            for v in &variants {
                if let Some(ty) = v.types.get(i) {
                    if is_variadic(ty) {
                        has_variadic = true;
                    }
                    if !seen_types.iter().any(|s| *s == *ty) {
                        seen_types.push(ty);
                    }
                }
            }
            let type_expr = if seen_types.len() == 1 {
                let ty = seen_types[0];
                quote! { <#ty as ::shingetsu::LuaTyped>::lua_type() }
            } else {
                let exprs: Vec<TokenStream> = seen_types
                    .iter()
                    .map(|ty| quote! { <#ty as ::shingetsu::LuaTyped>::lua_type() })
                    .collect();
                quote! { ::shingetsu::LuaType::Union(::std::vec![#(#exprs),*]) }
            };
            if i >= min_arity && !has_variadic {
                pos_type_exprs.push(quote! {
                    ::shingetsu::LuaType::Optional(::std::boxed::Box::new(#type_expr))
                });
            } else {
                pos_type_exprs.push(type_expr);
            }

            // Longest variant's name wins for this position;
            // matches conventional doc style for overloads like
            // `table.insert(list, [pos,] value)`.
            let mut chosen: Option<String> = None;
            for v in &variants {
                if let Some(Some(n)) = v.names.get(i) {
                    chosen = Some(n.clone());
                    break;
                }
            }
            let name_expr = match chosen {
                Some(n) => quote! { ::std::option::Option::Some(#n) },
                None => quote! { ::std::option::Option::None },
            };
            pos_name_exprs.push(name_expr);
        }
    }

    quote! {
        impl ::shingetsu::FromLuaMulti for #name {
            fn from_lua_multi(__vals: ::shingetsu::ValueVec) -> ::std::result::Result<Self, ::shingetsu::VmError> {
                let __n = __vals.len();
                let mut __last_err: ::std::option::Option<::shingetsu::VmError> = ::std::option::Option::None;
                #(#arms)*
                if let ::std::option::Option::Some(__e) = __last_err {
                    return Err(__e);
                }
                let __msg = if #min_arity == #max_arity {
                    ::std::format!("expected {} arguments but got {}", #min_arity, __n)
                } else if __n < #min_arity {
                    ::std::format!("expected at least {} arguments but got {}", #min_arity, __n)
                } else {
                    ::std::format!("expected at most {} arguments but got {}", #max_arity, __n)
                };
                Err(::shingetsu::VmError::LuaError {
                    display: __msg.clone(),
                    value: ::shingetsu::Value::String(
                        ::shingetsu::Bytes::from(__msg),
                    ),
                })
            }
        }

        impl ::shingetsu::LuaTypedMulti for #name {
            fn lua_types() -> ::std::vec::Vec<::shingetsu::LuaType> {
                ::std::vec![#(#pos_type_exprs),*]
            }
            fn lua_param_names() -> ::std::vec::Vec<::std::option::Option<&'static str>> {
                ::std::vec![#(#pos_name_exprs),*]
            }
        }
    }
}
