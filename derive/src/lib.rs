//! Derive macros for `samara`. Use them through the `samara` crate.
use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::{Data, DeriveInput, Fields, parse_macro_input, spanned::Spanned};

/// Implements `samara::Io` for a struct with named fields.
///
/// Each field becomes one freshly created capability, labelled with the
/// field's name. Field types must themselves implement `Io`:
/// `Effect<D>`, `Source<S>`, `File<P>`, a tuple of those, or another struct
/// that derives it. The struct must also be `Clone`.
#[proc_macro_derive(Io)]
pub fn derive_io(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            other => {
                return syn::Error::new(
                    other.span(),
                    "Io can only be derived for a struct with named fields; \
                     use a tuple type directly for unnamed capabilities",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new(
                input.ident.span(),
                "Io can only be derived for a struct with named fields",
            )
            .to_compile_error()
            .into();
        }
    };

    let declarations = fields.iter().map(|field| {
        let ident = field.ident.as_ref().expect("named field");
        let label = ident.to_string();
        // Spanned to the field's type so an unsupported type is reported there.
        quote_spanned! { field.ty.span() =>
            #ident: new.field(#label)
        }
    });

    quote! {
        impl #impl_generics ::samara::Io for #name #type_generics #where_clause {
            fn declare(new: &mut ::samara::Declaring<'_>) -> Self {
                Self { #(#declarations,)* }
            }
        }
    }
    .into()
}

/// Implements `samara::Connections` for a struct with named fields.
///
/// Each field's connections are listed under the field's name. Field types
/// must themselves implement `Connections`: `Connector<M>`, `Port<M>`,
/// `ComponentRef<C>`, an `Option`, `Vec`, or tuple of those, or another
/// struct that derives it.
#[proc_macro_derive(Connections)]
pub fn derive_connections(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            other => {
                return syn::Error::new(
                    other.span(),
                    "Connections can only be derived for a struct with named fields; \
                     use a tuple type directly for unnamed connections",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new(
                input.ident.span(),
                "Connections can only be derived for a struct with named fields",
            )
            .to_compile_error()
            .into();
        }
    };

    let listings = fields.iter().map(|field| {
        let ident = field.ident.as_ref().expect("named field");
        let label = ident.to_string();
        // Spanned to the field's type so an unsupported type is reported there.
        quote_spanned! { field.ty.span() =>
            into.field(#label, &self.#ident);
        }
    });

    quote! {
        impl #impl_generics ::samara::Connections for #name #type_generics #where_clause {
            fn list(&self, into: &mut ::samara::Listing<'_>) {
                #(#listings)*
            }
        }
    }
    .into()
}
