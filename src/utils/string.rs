pub fn trim_suffix(string: String, suffix: char) -> String {
    match string.strip_suffix(suffix) {
        Some(s) => s.to_string(),
        None => string,
    }
}
