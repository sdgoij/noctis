//! Derive and attribute macros for the Noctis `ecs` crate.
//!
//! `#[derive(Component)]` implements `ecs::reflect::Component` for a struct with
//! named fields: it infers each field's editor kind from its Rust type and
//! generates `references` from the fields marked `#[field(asset)]`.
//!
//! `#[plugin]` marks a project's [`Plugin`](ecs::app::Plugin) type and exports the
//! `<crate>::noctis_plugin` symbol the tool links.
//!
//! The struct still needs `Serialize`, `Deserialize`, and `Default` (the trait's
//! supertraits), which are derived separately:
//!
//! ```ignore
//! use ecs::prelude::*;
//!
//! #[derive(Debug, Default, Serialize, Deserialize, Component)]
//! struct Health {
//!     current: f32,
//!     max: f32,
//!     #[field(asset)]
//!     icon: Option<String>,
//! }
//! ```

#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields, ItemStruct, Lit, Type, parse_macro_input};

/// Derive `ecs::reflect::Component`.
///
/// `NAME` is the struct's name, or `#[component(name = "...")]`. Each named field
/// contributes a [`Field`](ecs::reflect::Field): `f32`/`f64` are numbers, the
/// integer types are integers, `bool` is a boolean, `String` is text, `[f32; 3]`
/// is a vector, and `[f32; 4]` is a colour; anything else defaults to text.
///
/// - `#[field(skip)]` omits a field from the schema.
/// - `#[field(number | integer | boolean | text | vec3 | color)]` overrides the kind.
/// - `#[field(asset)]` marks an asset path (`String` or `Option<String>`); its
///   value is returned from `references`, so the bundler ships the file it names.
/// - `#[field(options = "a, b, c")]` makes it an enumerated field.
#[proc_macro_derive(Component, attributes(component, field))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// One field's generated reflection.
struct FieldSpec {
    ident: syn::Ident,
    name: String,
    kind: TokenStream2,
    asset: bool,
    optional: bool,
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let component_name = component_name(input)?;
    let ecs = ecs_path();

    let fields: Vec<FieldSpec> = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => named
                .named
                .iter()
                .map(field_spec)
                .collect::<syn::Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect(),
            _ => {
                return Err(syn::Error::new_spanned(
                    input,
                    "Component requires a struct with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "Component can only be derived for structs",
            ));
        }
    };

    let entries = fields.iter().map(|field| {
        let FieldSpec { name, kind, .. } = field;
        quote! { #ecs::reflect::Field { name: #name, kind: #ecs::reflect::#kind } }
    });

    let references = fields
        .iter()
        .filter(|field| field.asset)
        .map(|field| {
            let FieldSpec {
                ident, optional, ..
            } = field;
            if *optional {
                quote! { out.extend(::std::option::Option::iter(&self.#ident).cloned()); }
            } else {
                quote! { out.push(self.#ident.clone()); }
            }
        })
        .collect::<Vec<_>>();

    Ok(quote! {
        impl #ecs::reflect::Component for #name {
            const NAME: &'static str = #component_name;

            fn fields() -> ::std::vec::Vec<#ecs::reflect::Field> {
                ::std::vec![ #(#entries),* ]
            }

            fn references(&self) -> ::std::vec::Vec<::std::string::String> {
                let mut out = ::std::vec::Vec::new();
                #(#references)*
                out
            }
        }
    })
}

/// Attribute: export a project's [`Plugin`](ecs::app::Plugin) under the
/// conventional name the `noctis` tool links.
///
/// Apply it to the plugin's type — a unit struct, or one implementing `Default`.
/// It keeps the type and appends the symbol a project's `Cargo.toml` lets the
/// tool find (`<crate>::noctis_plugin`):
///
/// ```ignore
/// use noctis::prelude::*;
///
/// #[noctis::plugin]
/// pub struct MyGame;
///
/// impl Plugin for MyGame {
///     fn register(&self, components: &mut ComponentRegistry) {
///         components.register::<Health>();
///     }
///     fn build(&self, app: &mut App) {
///         app.on_update("regen", |ctx| { /* ... */ });
///     }
/// }
/// ```
///
/// The behaviour — the components and systems — is written by hand in an
/// `impl Plugin`; this attribute only establishes the exported symbol, so the
/// tool needs no manifest entry to locate the plugin.
#[proc_macro_attribute]
pub fn plugin(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemStruct);
    match expand_plugin(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Emit the plugin type plus the `noctis_plugin()` symbol.
fn expand_plugin(item: &ItemStruct) -> syn::Result<TokenStream2> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "a plugin type must not be generic",
        ));
    }
    let name = &item.ident;
    let ecs = ecs_path();
    // A unit struct is its own value; anything else must be `Default`.
    let construction = match &item.fields {
        Fields::Unit => quote!(#name),
        _ => quote!(<#name as ::core::default::Default>::default()),
    };
    Ok(quote! {
        #item

        /// The project's plugin, located by the `noctis` tool through the crate's
        /// `Cargo.toml`. Generated by `#[noctis::plugin]`; not called directly.
        #[doc(hidden)]
        pub fn noctis_plugin() -> impl #ecs::app::Plugin {
            #construction
        }
    })
}

/// The path to the `ecs` crate as seen by the deriving crate.
///
/// `ecs` for a direct consumer, `crate` inside `ecs` itself, and the re-exported
/// `::noctis::ecs` for a crate that depends only on the `noctis` facade (a project
/// plugin), so `#[derive(Component)]` works through `use noctis::prelude::*`.
fn ecs_path() -> TokenStream2 {
    match crate_name("ecs") {
        Ok(FoundCrate::Itself) => quote!(crate),
        Ok(FoundCrate::Name(name)) => {
            let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
            quote!(::#ident)
        }
        Err(_) => quote!(::noctis::ecs),
    }
}

/// The component's `NAME`: `#[component(name = "...")]`, else the struct's ident.
fn component_name(input: &DeriveInput) -> syn::Result<String> {
    for attr in &input.attrs {
        if !attr.path().is_ident("component") {
            continue;
        }
        let mut custom = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value = meta.value()?;
                let literal: syn::LitStr = value.parse()?;
                custom = Some(literal.value());
                Ok(())
            } else {
                Err(meta.error("unknown component attribute (expected `name`)"))
            }
        })?;
        if let Some(name) = custom {
            return Ok(name);
        }
    }
    Ok(input.ident.to_string())
}

/// One field's schema, or `None` when it is `#[field(skip)]`.
fn field_spec(field: &syn::Field) -> syn::Result<Option<FieldSpec>> {
    let ident = field
        .ident
        .clone()
        .expect("derive only runs on named fields");
    let name = ident.to_string();

    let mut kind: Option<TokenStream2> = None;
    let mut skip = false;
    let mut asset = false;

    for attr in &field.attrs {
        if !attr.path().is_ident("field") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            match meta
                .path
                .get_ident()
                .map(|ident| ident.to_string())
                .as_deref()
            {
                Some("skip") => skip = true,
                Some("asset") => {
                    asset = true;
                    kind = Some(quote!(FieldKind::Asset));
                }
                Some("number") => kind = Some(quote!(FieldKind::Number)),
                Some("integer") => kind = Some(quote!(FieldKind::Integer)),
                Some("boolean") => kind = Some(quote!(FieldKind::Boolean)),
                Some("text") => kind = Some(quote!(FieldKind::Text)),
                Some("vec3") => kind = Some(quote!(FieldKind::Vec3)),
                Some("color") => kind = Some(quote!(FieldKind::Color)),
                Some("options") => {
                    let value = meta.value()?;
                    let literal: syn::LitStr = value.parse()?;
                    let options = literal
                        .value()
                        .split(',')
                        .map(|option| syn::LitStr::new(option.trim(), literal.span()))
                        .collect::<Vec<_>>();
                    kind = Some(quote!(FieldKind::Enum(&[#(#options),*])));
                }
                _ => return Err(meta.error("unknown field attribute")),
            }
            Ok(())
        })?;
    }

    if skip {
        return Ok(None);
    }

    let (inferred, is_string, is_option_string) = infer_kind(&field.ty);
    if asset && !is_string && !is_option_string {
        return Err(syn::Error::new_spanned(
            &field.ty,
            "an asset field must be a `String` or `Option<String>`",
        ));
    }
    Ok(Some(FieldSpec {
        ident,
        name,
        kind: kind.unwrap_or(inferred),
        asset,
        optional: is_option_string,
    }))
}

/// The inferred `FieldKind` expression, plus whether the type is `String` /
/// `Option<String>` (which `references` needs for an asset field).
fn infer_kind(ty: &Type) -> (TokenStream2, bool, bool) {
    if let Type::Array(array) = ty
        && is_named(&array.elem, "f32")
        && let syn::Expr::Lit(syn::ExprLit {
            lit: Lit::Int(len), ..
        }) = &array.len
        && let Ok(size) = len.base10_parse::<usize>()
    {
        match size {
            3 => return (quote!(FieldKind::Vec3), false, false),
            4 => return (quote!(FieldKind::Color), false, false),
            _ => {}
        }
    }

    if let Type::Path(path) = ty
        && let Some(segment) = path.path.segments.last()
    {
        match segment.ident.to_string().as_str() {
            "f32" | "f64" => return (quote!(FieldKind::Number), false, false),
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
            | "u128" | "usize" => {
                return (quote!(FieldKind::Integer), false, false);
            }
            "bool" => return (quote!(FieldKind::Boolean), false, false),
            "String" | "str" => return (quote!(FieldKind::Text), true, false),
            "Option" => {
                if let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments
                    && let Some(syn::GenericArgument::Type(inner)) = arguments.args.first()
                    && is_named(inner, "String")
                {
                    return (quote!(FieldKind::Text), false, true);
                }
            }
            _ => {}
        }
    }

    (quote!(FieldKind::Text), false, false)
}

/// Whether `ty` is a path whose last segment is `name`.
fn is_named(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|s| s.ident == name))
}
