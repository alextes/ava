/// escape text for telegram HTML mode
/// escapes <, >, and & characters
#[allow(dead_code)]
pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("hello"), "hello");
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
        assert_eq!(escape_html("a & b"), "a &amp; b");
        assert_eq!(escape_html("1 < 2 > 0"), "1 &lt; 2 &gt; 0");
    }
}
