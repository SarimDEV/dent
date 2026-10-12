//! Generates the Swift bindings from the built library (decision 0023: our
//! own script, not a third-party wrapper).

fn main() {
    uniffi::uniffi_bindgen_main()
}
