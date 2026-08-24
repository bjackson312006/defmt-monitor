//! Proc macro implementation for [`defmt-monitor`]. Use that crate, not this one.
//!
//! [`defmt-monitor`]: https://docs.rs/defmt-monitor

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, LitStr, Token};

/// Prefix identifying a monitor frame, and the version of the wire format.
///
/// Must match `defmt_monitor::SENTINEL`.
const SENTINEL: &str = "[MON1]";

struct MonitorInput {
    topic: LitStr,
    spec: LitStr,
    args: Punctuated<Expr, Token![,]>,
}

impl Parse for MonitorInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let topic = input.parse::<LitStr>().map_err(|e| {
            syn::Error::new(e.span(), "expected a string literal topic, e.g. \"imu/accel/x\"")
        })?;
        input.parse::<Token![,]>()?;
        let spec = input.parse::<LitStr>().map_err(|e| {
            syn::Error::new(e.span(), "expected a defmt format string literal, e.g. \"{=f32}\"")
        })?;

        let args = if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            Punctuated::parse_terminated(input)?
        } else {
            Punctuated::new()
        };

        Ok(MonitorInput { topic, spec, args })
    }
}

/// Implementation of `defmt_monitor::monitor!`.
#[proc_macro]
pub fn monitor(input: TokenStream) -> TokenStream {
    let MonitorInput { topic, spec, args } = syn::parse_macro_input!(input as MonitorInput);

    let topic_value = topic.value();
    if let Err(msg) = validate_topic(&topic_value) {
        return syn::Error::new(topic.span(), msg).to_compile_error().into();
    }

    // Build the defmt format string here so that defmt's own proc macro receives a
    // genuine string literal token. The topic is therefore interned into the ELF's
    // `.defmt` section at compile time and costs zero bytes on the wire.
    let fmt = LitStr::new(
        &format!("{SENTINEL}[{topic_value}][{}]", spec.value()),
        spec.span(),
    );

    let args = args.iter();
    let call = quote!(#fmt #(, #args)*);

    // `defmt`'s macros expand to bare `defmt::export::*` paths, so they resolve against
    // the *calling* crate's `defmt` dependency. Emitting `::defmt::` here keeps this
    // crate agnostic to which defmt version the firmware uses.
    let expanded = if cfg!(feature = "level-error") {
        quote!(::defmt::error!(#call))
    } else {
        quote!(::defmt::info!(#call))
    };
    expanded.into()
}

/// Rejects topics that would make the emitted format string ambiguous to parse, or
/// that defmt would misinterpret as containing format arguments.
fn validate_topic(topic: &str) -> Result<(), String> {
    if topic.is_empty() {
        return Err("monitor topic must not be empty".to_string());
    }
    for c in ['[', ']', '{', '}'] {
        if topic.contains(c) {
            return Err(format!(
                "monitor topic must not contain `{c}`; it would make the frame ambiguous \
                 to the host parser. Use `/` to separate topic segments."
            ));
        }
    }
    Ok(())
}
