use regex::RegexBuilder;

pub fn extract_xml_files(raw: &str) -> Vec<(String, String)> {
    let re = RegexBuilder::new(r#"<file\s+path="([^"]+)">\s*(.*?)\s*</file>"#)
        .dot_matches_new_line(true)
        .build()
        .expect("valid regex");

    re.captures_iter(raw)
        .map(|cap| (cap[1].to_string(), cap[2].trim().to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_multiple_xml_files() {
        let raw = r#"
Here is the plan:

<file path="src/main.rs">
fn main() {
    println!("Hello");
}
</file>

And the lib:
<file path="src/lib.rs">pub fn add(a: i32, b: i32) -> i32 { a + b }</file>
        "#;

        let files = extract_xml_files(raw);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "src/main.rs");
        assert!(files[0].1.contains("println!"));
        assert_eq!(files[1].0, "src/lib.rs");
        assert_eq!(files[1].1, "pub fn add(a: i32, b: i32) -> i32 { a + b }");
    }
}
