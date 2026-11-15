//! Derive macros for `samara`. Use them through the `samara` crate.
use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::{Data, DeriveInput, Fields, parse_macro_input, spanned::Spanned};

/// Implements `samara::Capabilities` for a struct with named fields.
///
/// Each field becomes one freshly created capability, labelled with the
/// field's name. Field types must themselves implement `Capabilities`:
/// `Effect<D>`, `Source<S>`, `File<P>`, a tuple of those, or another struct
/// that derives it. The struct must also be `Clone`.
#[proc_macro_derive(Capabilities)]
pub fn derive_capabilities(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            other => {
                return syn::Error::new(
                    other.span(),
                    "Capabilities can only be derived for a struct with named fields; \
                     use a tuple type directly for unnamed capabilities",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new(
                input.ident.span(),
                "Capabilities can only be derived for a struct with named fields",
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
        impl #impl_generics ::samara::Capabilities for #name #type_generics #where_clause {
            fn declare(new: &mut ::samara::Declaring<'_>) -> Self {
                Self { #(#declarations,)* }
            }
        }
    }
    .into()
}
