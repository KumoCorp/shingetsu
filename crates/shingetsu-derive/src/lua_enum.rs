//! `derive(FromLua)` and `derive(IntoLua)` for enums with newtype variants.
//!
//! Each variant must be a single-field tuple variant (newtype).  The
//! generated `FromLua` tries each variant's inner `FromLua` in an
//! order determined by discriminant-set analysis — narrower types are
//! tried first so that e.g. `i64` is attempted before `f64`.
//!
//! The generated `LuaTyped` produces a `LuaType::Union` of the inner
//! types, which flows into `ParamSpec.lua_type` for type-checking
//! tooling.

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
                "i64" | "i32" | "u32" | "usize" => Ok(DiscriminantSet(DiscriminantSet::INTEGER)),
                "f64" | "f32" => Ok(DiscriminantSet(
                    DiscriminantSet::INTEGER | DiscriminantSet::FLOAT,
                )),
                "Bytes" | "String" => Ok(DiscriminantSet(DiscriminantSet::STRING)),
                "Table" | "Vec" | "HashMap" | "BTreeMap" => {
                    Ok(DiscriminantSet(DiscriminantSet::TABLE))
                }
                "Function" => Ok(DiscriminantSet(DiscriminantSet::FUNCTION)),
                "Arc" => Ok(DiscriminantSet(DiscriminantSet::USERDATA)),
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

/// Lua-facing type name for the `expected` field in error messages.
fn lua_type_name(ty: &Type) -> &'static str {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            let name = seg.ident.to_string();
            return match name.as_str() {
                "bool" => "boolean",
                "i64" | "i32" | "u32" | "usize" => "integer",
                "f64" | "f32" => "number",
                "Bytes" | "String" => "string",
                "Table" | "Vec" | "HashMap" | "BTreeMap" => "table",
                "Function" => "function",
                "Arc" => "userdata",
                "Value" => "any",
                _ => "table",
            };
        }
    }
    "table"
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

    // Build the `expected` string: "integer | function | ..."
    // Use sorted order so it matches the try order.
    let expected_parts: Vec<&str> = variants.iter().map(|v| lua_type_name(v.ty)).collect();
    let expected_str = expected_parts.join(" | ");

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

    // LuaTyped: Union of all variant inner types.
    let type_exprs: Vec<TokenStream> = variants
        .iter()
        .map(|v| {
            let ty = v.ty;
            quote! { <#ty as ::shingetsu::LuaTyped>::lua_type() }
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
                    expected: #expected_str.to_owned(),
                    got: __type_name.to_owned(),
                })
            }
        }

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
