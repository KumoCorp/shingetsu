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
use syn::{DeriveInput, Fields, Type};

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
                "Arc" | "Ud" => Ok(DiscriminantSet(DiscriminantSet::USERDATA)),
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

struct VariantInfo<'a> {
    ident: &'a syn::Ident,
    ty: &'a Type,
    discs: DiscriminantSet,
}

fn collect_variants(data: &syn::DataEnum) -> syn::Result<Vec<VariantInfo<'_>>> {
    let mut out = Vec::new();
    for variant in &data.variants {
        match &variant.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let field = fields.unnamed.first().expect("checked len == 1");
                let discs = discriminant_set(&field.ty)
                    .map_err(|msg| syn::Error::new_spanned(&field.ty, msg))?;
                out.push(VariantInfo {
                    ident: &variant.ident,
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
fn sort_and_validate(variants: &mut [VariantInfo<'_>]) -> syn::Result<()> {
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

    // Check for identical discriminant sets (always an error).
    for i in 0..variants.len() {
        for j in (i + 1)..variants.len() {
            if variants[i].discs == variants[j].discs {
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
    let mut variants = match collect_variants(data) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    if variants.is_empty() {
        return syn::Error::new_spanned(name, "FromLua derive requires at least one variant")
            .to_compile_error();
    }

    if let Err(e) = sort_and_validate(&mut variants) {
        return e.to_compile_error();
    }

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
                // Last variant: consume the value (no clone).
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

    quote! {
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
    }
}

// ---------------------------------------------------------------------------
// derive(LuaTyped) for enums
// ---------------------------------------------------------------------------

pub fn derive_enum_lua_typed(parsed: &DeriveInput, data: &syn::DataEnum) -> TokenStream {
    let name = &parsed.ident;
    let variants = match collect_variants(data) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

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
            quote! {
                #name::#variant_ident(inner) => ::shingetsu::IntoLua::into_lua(inner),
            }
        })
        .collect();

    quote! {
        impl ::shingetsu::IntoLua for #name {
            fn into_lua(self) -> ::shingetsu::Value {
                match self {
                    #(#arms)*
                }
            }
        }
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

    // Collect (variant_ident, field_types, arity) and sort by descending arity
    // so we try the longest match first.
    let mut variants: Vec<(&syn::Ident, Vec<&syn::Type>, usize)> = Vec::new();
    for v in &data.variants {
        match &v.fields {
            Fields::Unnamed(fields) => {
                let types: Vec<&syn::Type> = fields.unnamed.iter().map(|f| &f.ty).collect();
                let arity = types.len();
                variants.push((&v.ident, types, arity));
            }
            Fields::Unit => {
                variants.push((&v.ident, Vec::new(), 0));
            }
            Fields::Named(_) => {
                return syn::Error::new_spanned(
                    &v.ident,
                    "FromLuaMulti derive does not support struct variants",
                )
                .to_compile_error();
            }
        }
    }
    variants.sort_by(|a, b| b.2.cmp(&a.2));

    // Generate match arms: try each variant starting from the longest.
    // For each variant, if the arg count matches, try FromLua on each field.
    // If any conversion fails, fall through to the next variant.
    let mut arms = Vec::<TokenStream>::new();
    for (variant_ident, field_types, arity) in &variants {
        if *arity == 0 {
            arms.push(quote! {
                if __n == 0 {
                    return Ok(#name::#variant_ident);
                }
            });
            continue;
        }
        let field_idents: Vec<syn::Ident> = (0..*arity)
            .map(|i| quote::format_ident!("__f{}", i))
            .collect();

        let extraction_bindings: Vec<TokenStream> = field_types
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                let fid = &field_idents[i];
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
            })
            .collect();

        arms.push(quote! {
            if __n == #arity {
                #(#extraction_bindings)*
                return Ok(#name::#variant_ident( #(#field_idents),* ));
            }
        });
    }

    // Build LuaTypedMulti: for each parameter position, union the types
    // across all variants.  Positions beyond a shorter variant are Optional.
    let max_arity = variants.iter().map(|(_, _, a)| *a).max().unwrap_or(0);
    let min_arity = variants.iter().map(|(_, _, a)| *a).min().unwrap_or(0);
    let mut pos_type_exprs = Vec::<TokenStream>::new();
    for i in 0..max_arity {
        // Collect the distinct types at position i across variants.
        let mut seen_types = Vec::<&syn::Type>::new();
        for (_, field_types, _) in &variants {
            if let Some(ty) = field_types.get(i) {
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
        if i >= min_arity {
            pos_type_exprs.push(quote! {
                ::shingetsu::LuaType::Optional(::std::boxed::Box::new(#type_expr))
            });
        } else {
            pos_type_exprs.push(type_expr);
        }
    }

    quote! {
        impl ::shingetsu::FromLuaMulti for #name {
            fn from_lua_multi(__vals: ::shingetsu::ValueVec) -> ::std::result::Result<Self, ::shingetsu::VmError> {
                let __n = __vals.len();
                #(#arms)*
                {
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
        }

        impl ::shingetsu::LuaTypedMulti for #name {
            fn lua_types() -> ::std::vec::Vec<::shingetsu::LuaType> {
                ::std::vec![#(#pos_type_exprs),*]
            }
        }
    }
}
