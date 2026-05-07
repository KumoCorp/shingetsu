//! `derive(FromLua)`, `derive(IntoLua)`, `derive(LuaTyped)`, and
//! `derive(LuaTable)` for struct ↔ Lua table conversion.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, LitStr, Type};

use crate::util::type_is;

// ---------------------------------------------------------------------------
// #[lua(...)] container attribute parsing
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ContainerOpts {
    /// Read this struct as `T` from lua, then `Self::try_from(T)`.
    try_from: Option<syn::Type>,
    /// IntoLua: convert via `Into<T>` before emitting.
    into: Option<syn::Type>,
    /// Whole-struct default applied when the lua value is `Nil`.
    /// `None` = no default, `Some(None)` = `Default::default()`,
    /// `Some(Some(path))` = call the named function.
    default: Option<Option<syn::Path>>,
    /// Reject lua tables containing keys not declared on the struct.
    deny_unknown_fields: bool,
}

fn parse_container_opts(attrs: &[syn::Attribute]) -> syn::Result<ContainerOpts> {
    let mut opts = ContainerOpts::default();
    for attr in attrs {
        if !attr.path().is_ident("lua") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("try_from") {
                let val: LitStr = meta.value()?.parse()?;
                opts.try_from = Some(syn::parse_str(&val.value())?);
                Ok(())
            } else if meta.path.is_ident("into") {
                let val: LitStr = meta.value()?.parse()?;
                opts.into = Some(syn::parse_str(&val.value())?);
                Ok(())
            } else if meta.path.is_ident("default") {
                if let Ok(value) = meta.value() {
                    let lit: LitStr = value.parse()?;
                    opts.default = Some(Some(syn::parse_str(&lit.value())?));
                } else {
                    opts.default = Some(None);
                }
                Ok(())
            } else if meta.path.is_ident("deny_unknown_fields") {
                opts.deny_unknown_fields = true;
                Ok(())
            } else {
                Err(meta.error(
                    "unknown lua container option; expected one of \
                     `try_from`, `into`, `default`, `deny_unknown_fields`",
                ))
            }
        })?;
    }
    if (opts.try_from.is_some() || opts.into.is_some()) && opts.deny_unknown_fields {
        // try_from/into delegate the whole struct to a different type, so
        // unknown-field checking is the intermediate type's responsibility.
        let span = attrs
            .iter()
            .find(|a| a.path().is_ident("lua"))
            .map(|a| a.path().span())
            .unwrap_or_else(proc_macro2::Span::call_site);
        return Err(syn::Error::new(
            span,
            "`deny_unknown_fields` is incompatible with container `try_from` / `into`",
        ));
    }
    Ok(opts)
}

// ---------------------------------------------------------------------------
// #[lua(...)] field attribute parsing
// ---------------------------------------------------------------------------

struct FieldOpts {
    /// Override the Lua key name.
    rename: Option<String>,
    /// Default expression when the field is nil/absent.
    default: Option<syn::Expr>,
    /// Field is omitted from FromLua, IntoLua, and LuaTyped.  The
    /// FromLua-side value is `T::default()`.
    skip: bool,
    /// Inline the inner struct's fields at this level (struct-typed only).
    flatten: bool,
    /// Read this field as `T` from lua, then `<FieldType>::try_from(T)`.
    /// Symmetric IntoLua uses `Into<T>`.
    try_from: Option<syn::Type>,
    /// IntoLua: convert via `Into<T>` before writing to the lua table.
    into: Option<syn::Type>,
    /// Deprecation reason recorded in field metadata for the
    /// type-checker lint (Phase 1).  Currently parsed and stored only.
    deprecated: Option<String>,
    /// Path to a validator `fn(&T) -> Result<(), impl Display>` invoked
    /// after FromLua extraction.
    validate: Option<syn::Path>,
}

fn parse_field_opts(attrs: &[syn::Attribute]) -> syn::Result<FieldOpts> {
    let mut opts = FieldOpts {
        rename: None,
        default: None,
        skip: false,
        flatten: false,
        try_from: None,
        into: None,
        deprecated: None,
        validate: None,
    };
    for attr in attrs {
        if !attr.path().is_ident("lua") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let val: LitStr = meta.value()?.parse()?;
                opts.rename = Some(val.value());
                Ok(())
            } else if meta.path.is_ident("default") {
                // `default` may be `default` (bare flag → T::default()) or
                // `default = expr`.
                if let Ok(value) = meta.value() {
                    let expr: syn::Expr = value.parse()?;
                    opts.default = Some(expr);
                } else {
                    opts.default = Some(syn::parse_quote!(::core::default::Default::default()));
                }
                Ok(())
            } else if meta.path.is_ident("skip") {
                opts.skip = true;
                Ok(())
            } else if meta.path.is_ident("flatten") {
                opts.flatten = true;
                Ok(())
            } else if meta.path.is_ident("try_from") {
                let val: LitStr = meta.value()?.parse()?;
                opts.try_from = Some(syn::parse_str(&val.value())?);
                Ok(())
            } else if meta.path.is_ident("into") {
                let val: LitStr = meta.value()?.parse()?;
                opts.into = Some(syn::parse_str(&val.value())?);
                Ok(())
            } else if meta.path.is_ident("deprecated") {
                let val: LitStr = meta.value()?.parse()?;
                opts.deprecated = Some(val.value());
                Ok(())
            } else if meta.path.is_ident("validate") {
                let val: LitStr = meta.value()?.parse()?;
                opts.validate = Some(syn::parse_str(&val.value())?);
                Ok(())
            } else {
                Err(meta.error(
                    "unknown lua field option; expected one of \
                     `rename`, `default`, `skip`, `flatten`, `try_from`, \
                     `into`, `deprecated`, `validate`",
                ))
            }
        })?;
    }
    validate_field_opts(&opts, attrs)?;
    Ok(opts)
}

fn validate_field_opts(opts: &FieldOpts, attrs: &[syn::Attribute]) -> syn::Result<()> {
    let span = attrs
        .iter()
        .find(|a| a.path().is_ident("lua"))
        .map(|a| a.path().span())
        .unwrap_or_else(proc_macro2::Span::call_site);
    if opts.flatten && (opts.try_from.is_some() || opts.into.is_some()) {
        return Err(syn::Error::new(
            span,
            "`flatten` is incompatible with `try_from` / `into`",
        ));
    }
    if opts.flatten && opts.rename.is_some() {
        return Err(syn::Error::new(
            span,
            "`flatten` does not use a key, so `rename` is meaningless",
        ));
    }
    if opts.skip && (opts.flatten || opts.try_from.is_some() || opts.into.is_some()) {
        return Err(syn::Error::new(
            span,
            "`skip` is incompatible with other conversion options",
        ));
    }
    Ok(())
}

/// Returns `true` if `ty` is `Option<T>`.
fn is_option(ty: &Type) -> bool {
    type_is(ty, "Option")
}

// ---------------------------------------------------------------------------
// Shared: collect field info
// ---------------------------------------------------------------------------

struct FieldInfo<'a> {
    ident: &'a syn::Ident,
    ty: &'a Type,
    lua_key: String,
    opts: FieldOpts,
}

fn collect_fields(fields: &Fields) -> syn::Result<Vec<FieldInfo<'_>>> {
    match fields {
        Fields::Named(named) => named
            .named
            .iter()
            .map(|f| {
                let ident = f.ident.as_ref().expect("named field");
                let opts = parse_field_opts(&f.attrs)?;
                let lua_key = opts.rename.clone().unwrap_or_else(|| ident.to_string());
                Ok(FieldInfo {
                    ident,
                    ty: &f.ty,
                    lua_key,
                    opts,
                })
            })
            .collect(),
        _ => Err(syn::Error::new_spanned(
            fields,
            "FromLua/IntoLua derive only supports structs with named fields",
        )),
    }
}

// ---------------------------------------------------------------------------
// Shared: generate LuaTyped field type specs
// ---------------------------------------------------------------------------

/// Produce the per-field token streams that contribute to the
/// `TableLuaType::fields` list.  A `flatten`ed field expands at runtime
/// into the inner struct's fields via `__flatten_into`.
fn gen_type_field_stmts(fields: &[FieldInfo<'_>]) -> Vec<TokenStream> {
    fields
        .iter()
        .filter(|f| !f.opts.skip)
        .map(|f| {
            if f.opts.flatten {
                let ty = f.ty;
                return quote! {
                    {
                        let __inner = <#ty as ::shingetsu::LuaTyped>::lua_type();
                        if let ::shingetsu::LuaType::Table(__t) = __inner {
                            for __pair in __t.fields {
                                __fields.push(__pair);
                            }
                        }
                    }
                };
            }

            let key = &f.lua_key;
            let key_bytes = key.as_bytes().to_vec();
            // The lua-facing type is the intermediate (try_from) type if
            // present, otherwise the field's own type.
            let surface_ty: TokenStream = match &f.opts.try_from {
                Some(t) => quote! { #t },
                None => {
                    let t = f.ty;
                    quote! { #t }
                }
            };
            let optional = f.opts.default.is_some() || is_option(f.ty);
            let lua_ty = if optional {
                quote! {
                    ::shingetsu::LuaType::Optional(
                        ::std::boxed::Box::new(<#surface_ty as ::shingetsu::LuaTyped>::lua_type())
                    )
                }
            } else {
                quote! { <#surface_ty as ::shingetsu::LuaTyped>::lua_type() }
            };
            quote! {
                __fields.push((
                    ::shingetsu::Bytes::from(&[ #(#key_bytes),* ][..]),
                    #lua_ty,
                ));
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// derive(FromLua)
// ---------------------------------------------------------------------------

pub fn derive_from_lua(input: TokenStream) -> TokenStream {
    let parsed: DeriveInput = match syn::parse2(input) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let name = &parsed.ident;
    let data = match &parsed.data {
        Data::Struct(s) => s,
        Data::Enum(e) => return crate::lua_enum::derive_enum_from_lua(&parsed, e),
        _ => {
            return syn::Error::new_spanned(name, "FromLua derive only supports structs and enums")
                .to_compile_error()
        }
    };

    let container = match parse_container_opts(&parsed.attrs) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    // Container-level try_from delegates the whole conversion.
    if let Some(intermediate) = &container.try_from {
        return quote! {
            impl ::shingetsu::FromLua for #name {
                fn from_lua(v: ::shingetsu::Value) -> ::std::result::Result<Self, ::shingetsu::VmError> {
                    let __interm: #intermediate =
                        <#intermediate as ::shingetsu::FromLua>::from_lua(v)?;
                    <#name as ::core::convert::TryFrom<#intermediate>>::try_from(__interm)
                        .map_err(|__e| ::shingetsu::VmError::BadArgument {
                            position: 0,
                            function: ::std::string::String::new(),
                            expected: ::std::format!("{} (try_from {})",
                                ::std::stringify!(#name),
                                ::std::stringify!(#intermediate)),
                            got: ::std::format!("{}", __e),
                        })
                }
            }
        };
    }

    let fields = match collect_fields(&data.fields) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let extractions: Vec<TokenStream> = fields.iter().map(gen_from_lua_field).collect();

    let field_names: Vec<&syn::Ident> = fields.iter().map(|f| f.ident).collect();

    let known_keys: Vec<TokenStream> = fields
        .iter()
        .filter(|f| !f.opts.skip && !f.opts.flatten)
        .map(|f| {
            let bytes = f.lua_key.as_bytes().to_vec();
            quote! { &[ #(#bytes),* ][..] }
        })
        .collect();
    let has_flatten = fields.iter().any(|f| f.opts.flatten);
    let deny_unknown_check = if container.deny_unknown_fields && !has_flatten {
        quote! {
            {
                let __known: &[&[u8]] = &[ #(#known_keys),* ];
                let mut __scan_key = ::shingetsu::Value::Nil;
                while let ::std::option::Option::Some((__k, _)) =
                    __table.next(&__scan_key)?
                {
                    if let ::shingetsu::Value::String(ref __sb) = __k {
                        let __bytes: &[u8] = __sb.as_ref();
                        if !__known.iter().any(|__n| *__n == __bytes) {
                            let __used = ::std::string::String::from_utf8_lossy(__bytes);
                            let __suggestion =
                                ::shingetsu::diagnostics::render_field_suggestion(
                                    &__used, __known
                                );
                            let __got = if __suggestion.is_empty() {
                                ::std::format!("unknown field `{}`", __used)
                            } else {
                                ::std::format!(
                                    "unknown field `{}`. {}",
                                    __used, __suggestion
                                )
                            };
                            return ::std::result::Result::Err(
                                ::shingetsu::VmError::BadArgument {
                                    position: 0,
                                    function: ::std::string::String::new(),
                                    expected: ::std::format!(
                                        "only known fields of {}",
                                        ::std::stringify!(#name)),
                                    got: __got,
                                });
                        }
                    }
                    __scan_key = __k;
                }
            }
        }
    } else {
        quote! {}
    };

    let nil_handler = match &container.default {
        Some(None) => quote! {
            ::shingetsu::Value::Nil => {
                return ::std::result::Result::Ok(
                    <Self as ::core::default::Default>::default());
            }
        },
        Some(Some(path)) => quote! {
            ::shingetsu::Value::Nil => {
                return ::std::result::Result::Ok(#path());
            }
        },
        None => quote! {},
    };

    quote! {
        impl ::shingetsu::FromLua for #name {
            fn from_lua(v: ::shingetsu::Value) -> ::std::result::Result<Self, ::shingetsu::VmError> {
                let __table = match v {
                    ::shingetsu::Value::Table(t) => t,
                    #nil_handler
                    other => {
                        return ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                            position: 0,
                            function: ::std::string::String::new(),
                            expected: "table".to_owned(),
                            got: other.type_name().to_owned(),
                        });
                    }
                };
                #deny_unknown_check
                #(#extractions)*
                ::std::result::Result::Ok(#name { #(#field_names),* })
            }
        }
    }
}

fn gen_from_lua_field(f: &FieldInfo<'_>) -> TokenStream {
    let ident = f.ident;
    let ty = f.ty;

    if f.opts.skip {
        return quote! {
            let #ident: #ty = ::core::default::Default::default();
        };
    }

    if f.opts.flatten {
        // The flattened type extracts itself from the same outer table.
        return quote! {
            let #ident: #ty = <#ty as ::shingetsu::FromLua>::from_lua(
                ::shingetsu::Value::Table(__table.clone())
            )?;
        };
    }

    let key = &f.lua_key;
    let validate = gen_validate(f);

    if let Some(intermediate) = &f.opts.try_from {
        let extract = if let Some(default_expr) = &f.opts.default {
            quote! {
                let __interm: ::std::option::Option<#intermediate> =
                    __table.get_field(#key)?;
                let __interm = match __interm {
                    ::std::option::Option::Some(v) => v,
                    ::std::option::Option::None => {
                        let #ident: #ty = #default_expr;
                        #validate
                        return ::std::result::Result::Ok::<_, ::shingetsu::VmError>(#ident);
                    }
                };
            }
        } else {
            quote! {
                let __interm: #intermediate = __table.get_field(#key)?;
            }
        };
        // Wrap in a closure so we can early-return the default value above.
        return quote! {
            let #ident: #ty = (|| -> ::std::result::Result<#ty, ::shingetsu::VmError> {
                #extract
                let #ident: #ty = <#ty as ::core::convert::TryFrom<#intermediate>>::try_from(__interm)
                    .map_err(|__e| ::shingetsu::VmError::BadArgument {
                        position: 0,
                        function: ::std::string::String::new(),
                        expected: ::std::format!("{} (try_from {})",
                            ::std::stringify!(#ty),
                            ::std::stringify!(#intermediate)),
                        got: ::std::format!("{}", __e),
                    })?;
                #validate
                ::std::result::Result::Ok(#ident)
            })()?;
        };
    }

    if let Some(default_expr) = &f.opts.default {
        return quote! {
            let #ident: #ty = match __table.get_field::<::std::option::Option<#ty>>(#key)? {
                ::std::option::Option::Some(v) => v,
                ::std::option::Option::None => #default_expr,
            };
            #validate
        };
    }

    quote! {
        let #ident: #ty = __table.get_field(#key)?;
        #validate
    }
}

fn gen_validate(f: &FieldInfo<'_>) -> TokenStream {
    let Some(path) = &f.opts.validate else {
        return quote! {};
    };
    let ident = f.ident;
    let key = &f.lua_key;
    quote! {
        if let ::std::result::Result::Err(__msg) = #path(&#ident) {
            return ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                position: 0,
                function: ::std::string::String::new(),
                expected: ::std::format!("validated {}", #key),
                got: ::std::format!("{}", __msg),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// derive(IntoLua)
// ---------------------------------------------------------------------------

pub fn derive_into_lua(input: TokenStream) -> TokenStream {
    let parsed: DeriveInput = match syn::parse2(input) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let name = &parsed.ident;
    let data = match &parsed.data {
        Data::Struct(s) => s,
        Data::Enum(e) => return crate::lua_enum::derive_enum_into_lua(&parsed, e),
        _ => {
            return syn::Error::new_spanned(name, "IntoLua derive only supports structs and enums")
                .to_compile_error()
        }
    };

    let container = match parse_container_opts(&parsed.attrs) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    if let Some(target) = &container.into.clone().or(container.try_from.clone()) {
        // No `LuaTableShape` impl: the intermediate type may produce a
        // non-table.  Users who know it does can `impl LuaTableShape`
        // by hand.
        return quote! {
            impl ::shingetsu::IntoLua for #name {
                fn into_lua(self) -> ::shingetsu::Value {
                    <#target as ::shingetsu::IntoLua>::into_lua(
                        ::core::convert::Into::<#target>::into(self)
                    )
                }
            }
        };
    }

    let fields = match collect_fields(&data.fields) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let insertions: Vec<TokenStream> = fields.iter().map(gen_into_lua_field).collect();

    quote! {
        impl ::shingetsu::IntoLua for #name {
            fn into_lua(self) -> ::shingetsu::Value {
                let __table = ::shingetsu::Table::new();
                #(#insertions)*
                ::shingetsu::Value::Table(__table)
            }
        }

        impl ::shingetsu::LuaTableShape for #name {
            fn into_lua_table(self) -> ::shingetsu::Table {
                let __table = ::shingetsu::Table::new();
                #(#insertions)*
                __table
            }
        }
    }
}

fn gen_into_lua_field(f: &FieldInfo<'_>) -> TokenStream {
    let ident = f.ident;

    if f.opts.skip {
        return quote! {};
    }

    if f.opts.flatten {
        let ty = f.ty;
        return quote! {
            {
                let __inner_value = <#ty as ::shingetsu::IntoLua>::into_lua(self.#ident);
                if let ::shingetsu::Value::Table(__inner) = __inner_value {
                    let mut __key = ::shingetsu::Value::Nil;
                    while let ::std::result::Result::Ok(
                        ::std::option::Option::Some((__k, __v))
                    ) = __inner.next(&__key) {
                        __table.raw_set(__k.clone(), __v).expect("table set");
                        __key = __k;
                    }
                }
            }
        };
    }

    let key_bytes = f.lua_key.as_bytes().to_vec();
    let key_expr = quote! {
        ::shingetsu::Value::String(
            ::shingetsu::Bytes::from(&[ #(#key_bytes),* ][..])
        )
    };

    // Apply `into = "T"` conversion before IntoLua.
    let value_expr = match (&f.opts.into, &f.opts.try_from) {
        (Some(target), _) => quote! {
            <#target as ::shingetsu::IntoLua>::into_lua(
                ::core::convert::Into::<#target>::into(self.#ident)
            )
        },
        // try_from implies a symmetric Into<T>.
        (None, Some(intermediate)) => quote! {
            <#intermediate as ::shingetsu::IntoLua>::into_lua(
                ::core::convert::Into::<#intermediate>::into(self.#ident)
            )
        },
        (None, None) => quote! {
            ::shingetsu::IntoLua::into_lua(self.#ident)
        },
    };

    if is_option(f.ty) && f.opts.into.is_none() && f.opts.try_from.is_none() {
        // Skip None values for plain Option fields; preserves existing behavior.
        return quote! {
            if let ::std::option::Option::Some(v) = self.#ident {
                __table.raw_set(
                    #key_expr,
                    ::shingetsu::IntoLua::into_lua(v),
                ).expect("table set");
            }
        };
    }

    quote! {
        __table.raw_set(#key_expr, #value_expr).expect("table set");
    }
}

// ---------------------------------------------------------------------------
// derive(LuaTyped)
// ---------------------------------------------------------------------------

pub fn derive_lua_typed(input: TokenStream) -> TokenStream {
    let parsed: DeriveInput = match syn::parse2(input) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let name = &parsed.ident;
    match &parsed.data {
        Data::Struct(s) => {
            let container = match parse_container_opts(&parsed.attrs) {
                Ok(v) => v,
                Err(e) => return e.to_compile_error(),
            };
            // Container try_from / into surfaces the intermediate's lua_type.
            if let Some(target) = container.try_from.as_ref().or(container.into.as_ref()) {
                return quote! {
                    impl ::shingetsu::LuaTyped for #name {
                        fn lua_type() -> ::shingetsu::LuaType {
                            <#target as ::shingetsu::LuaTyped>::lua_type()
                        }
                    }
                };
            }
            let fields = match collect_fields(&s.fields) {
                Ok(v) => v,
                Err(e) => return e.to_compile_error(),
            };
            let stmts = gen_type_field_stmts(&fields);
            quote! {
                impl ::shingetsu::LuaTyped for #name {
                    fn lua_type() -> ::shingetsu::LuaType {
                        let mut __fields: ::std::vec::Vec<(
                            ::shingetsu::Bytes,
                            ::shingetsu::LuaType,
                        )> = ::std::vec::Vec::new();
                        #(#stmts)*
                        ::shingetsu::LuaType::Table(::std::boxed::Box::new(
                            ::shingetsu::TableLuaType {
                                fields: __fields,
                                indexer: ::std::option::Option::None,
                            }
                        ))
                    }
                }
            }
        }
        Data::Enum(e) => crate::lua_enum::derive_enum_lua_typed(&parsed, e),
        _ => syn::Error::new_spanned(name, "LuaTyped derive only supports structs and enums")
            .to_compile_error(),
    }
}

// ---------------------------------------------------------------------------
// derive(LuaTable) — convenience for FromLua + IntoLua + LuaTyped
// ---------------------------------------------------------------------------

pub fn derive_lua_table(input: TokenStream) -> TokenStream {
    let from = derive_from_lua(input.clone());
    let into = derive_into_lua(input.clone());
    let typed = derive_lua_typed(input);
    quote! {
        #from
        #into
        #typed
    }
}
