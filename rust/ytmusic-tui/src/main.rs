fn main() {
    println!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    /// Verify that the package name and version strings are non-empty at build time.
    #[test]
    fn version_strings_are_non_empty() {
        let name = env!("CARGO_PKG_NAME");
        let version = env!("CARGO_PKG_VERSION");
        assert!(!name.is_empty(), "CARGO_PKG_NAME must not be empty");
        assert!(!version.is_empty(), "CARGO_PKG_VERSION must not be empty");
        assert_eq!(name, "ytmusic-tui");
    }
}
