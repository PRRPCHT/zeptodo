use std::collections::HashSet;
use std::sync::OnceLock;

use ammonia::Builder;
use pulldown_cmark::{Options, Parser, html};

/// Build the shared [`ammonia::Builder`] used to sanitize rendered Markdown.
///
/// ### Returns
/// - `&'static Builder<'static>`: A reusable, immutable sanitizer.
fn sanitizer() -> &'static Builder<'static> {
    static BUILDER: OnceLock<Builder<'static>> = OnceLock::new();
    BUILDER.get_or_init(|| {
        let mut schemes = HashSet::new();
        schemes.insert("http");
        schemes.insert("https");
        schemes.insert("mailto");

        let mut builder = Builder::default();
        builder
            .url_schemes(schemes)
            .link_rel(Some("noopener nofollow"));
        builder
    })
}

/// Render a Markdown source string to sanitized HTML.
///
/// ### Arguments
/// - `input`: The raw Markdown source.
///
/// ### Returns
/// - `String`: Safe HTML, ready to inject into a template with `|safe`.
pub fn render(input: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);

    let parser = Parser::new_ext(input, options);
    let mut unsafe_html = String::with_capacity(input.len());
    html::push_html(&mut unsafe_html, parser);

    sanitizer().clean(&unsafe_html).to_string()
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn strips_script_tags() {
        let html = render("Hello <script>alert(1)</script> world");
        assert!(!html.contains("<script"));
        assert!(!html.contains("alert(1)"));
        assert!(html.contains("Hello"));
        assert!(html.contains("world"));
    }

    #[test]
    fn strips_javascript_urls() {
        let html = render("[click](javascript:alert(1))");
        assert!(!html.to_lowercase().contains("javascript:"));
    }

    #[test]
    fn strips_inline_event_handlers() {
        let html = render("<img src=\"x\" onerror=\"alert(1)\">");
        assert!(!html.to_lowercase().contains("onerror"));
        assert!(!html.contains("alert(1)"));
    }

    #[test]
    fn keeps_safe_http_link() {
        let html = render("[home](https://example.com)");
        assert!(html.contains("href=\"https://example.com\""));
    }

    #[test]
    fn keeps_mailto_link() {
        let html = render("[mail](mailto:a@b.c)");
        assert!(html.contains("href=\"mailto:a@b.c\""));
    }

    #[test]
    fn renders_basic_formatting() {
        let html = render("**bold** and *italic*");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
    }

    #[test]
    fn renders_strikethrough() {
        let html = render("~~gone~~");
        assert!(html.contains("<del>gone</del>"));
    }

    #[test]
    fn renders_tables() {
        let html = render("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(html.contains("<table"));
        assert!(html.contains("<th>a</th>"));
        assert!(html.contains("<td>1</td>"));
    }

    #[test]
    fn escapes_raw_html_input_outside_allowlist() {
        let html = render("<iframe src=\"https://evil\"></iframe>");
        assert!(!html.contains("<iframe"));
    }
}
