/// Validates shipping Fluent resources without a graphical environment.
fn main() {
    match hunkra::i18n::check_catalogs() {
        Ok(()) => println!("Fluent: en/ru complete, overlay signatures and formatting valid"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
