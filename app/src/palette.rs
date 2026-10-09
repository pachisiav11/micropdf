//! Command list for the Ctrl+K palette, and the fuzzy matcher behind it.

pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    pub shortcut: &'static str,
}

const fn c(id: &'static str, title: &'static str, shortcut: &'static str) -> Command {
    Command {
        id,
        title,
        shortcut,
    }
}

pub const COMMANDS: &[Command] = &[
    c("open", "Open file", "Ctrl+O"),
    c("save", "Save", "Ctrl+S"),
    c("save-as", "Save as", "Ctrl+Shift+S"),
    c("close-tab", "Close tab", "Ctrl+W"),
    c("reopen-tab", "Reopen closed tab", "Ctrl+Shift+T"),
    c("next-tab", "Next tab", "Ctrl+Tab"),
    c("prev-tab", "Previous tab", "Ctrl+Shift+Tab"),
    c("find", "Find in document", "Ctrl+F"),
    c("library-search", "Search the library", "Ctrl+Shift+F"),
    c("library-add", "Library: add a folder", ""),
    c("library-remove", "Library: remove a folder", ""),
    c("library-update", "Library: read the folders again", ""),
    c("goto", "Go to page", "Ctrl+G"),
    c("first-page", "First page", "Home"),
    c("last-page", "Last page", "End"),
    c("next-page", "Next page", "Right"),
    c("prev-page", "Previous page", "Left"),
    c("back", "Back", "Alt+Left"),
    c("forward", "Forward", "Alt+Right"),
    c("zoom-in", "Zoom in", "Ctrl+="),
    c("zoom-out", "Zoom out", "Ctrl+-"),
    c("fit-width", "Fit width", "Ctrl+2"),
    c("fit-page", "Fit page", "Ctrl+0"),
    c("zoom-100", "Actual size", "Ctrl+1"),
    c("rotate-cw", "Rotate view clockwise", "Ctrl+Shift+="),
    c("rotate-ccw", "Rotate view counterclockwise", "Ctrl+Shift+-"),
    c("mode-single", "Layout: single page", ""),
    c("mode-continuous", "Layout: continuous", ""),
    c("mode-two-up", "Layout: two-up", ""),
    c("mode-book", "Layout: book (cover alone)", ""),
    c("read-normal", "Pages: normal colours", ""),
    c("read-dark", "Pages: dark", ""),
    c("read-sepia", "Pages: sepia", ""),
    c("read-invert", "Pages: inverted", ""),
    c("toggle-theme", "Switch light or dark theme", ""),
    c("toggle-vim", "Toggle Vim keys", ""),
    c("sidebar", "Toggle sidebar", "F4"),
    c("show-thumbs", "Show page thumbnails", ""),
    c("show-outline", "Show outline", ""),
    c("show-comments", "Show comments", ""),
    c("properties", "Document properties", "Ctrl+D"),
    c("assistant", "Assistant", "Ctrl+Shift+A"),
    c("ask-document", "Assistant: summarize the document", ""),
    c("ask-page", "Assistant: summarize this page", ""),
    c("ask-selection", "Assistant: explain the selection", ""),
    c("copy", "Copy selected text", "Ctrl+C"),
    c("select-all", "Select all text on this page", "Ctrl+A"),
    c("undo", "Undo", "Ctrl+Z"),
    c("redo", "Redo", "Ctrl+Y"),
    c("highlight", "Highlight selected text", ""),
    c("underline", "Underline selected text", ""),
    c("strikeout", "Strike out selected text", ""),
    c("color-yellow", "Comment colour: yellow", ""),
    c("color-red", "Comment colour: red", ""),
    c("color-green", "Comment colour: green", ""),
    c("color-blue", "Comment colour: blue", ""),
    c("reset-form", "Reset form fields", ""),
    c("export-form", "Export form data (XFDF)", ""),
    c("import-form", "Import form data (XFDF)", ""),
    c("flatten-form", "Flatten form fields into the page", ""),
    c("flatten-comments", "Flatten comments into the page", ""),
    c("export-comments", "Export comments (XFDF)", ""),
    c("summarize-comments", "Summarize comments (PDF)", ""),
    c("reply", "Reply to the selected comment", ""),
    c("status-accepted", "Comment status: accepted", ""),
    c("status-rejected", "Comment status: rejected", ""),
    c("status-cancelled", "Comment status: cancelled", ""),
    c("status-completed", "Comment status: completed", ""),
    c("status-none", "Comment status: none", ""),
    c("import-comments", "Import comments (XFDF)", ""),
    c("sign", "Add signature", ""),
    c("initials", "Add initials", ""),
    c("new-signature", "New signature", ""),
    c("new-initials", "New initials", ""),
    c("forget-signatures", "Forget saved signatures", ""),
    c("tool-callout", "Tool: callout", ""),
    c("stamp", "Add a stamp", ""),
    c("attach-file", "Attach a file to the document", ""),
    c("tool-attach", "Tool: attach a file to the page", ""),
    c("stamp-Approved", "Stamp: Approved", ""),
    c("stamp-NotApproved", "Stamp: Not approved", ""),
    c("stamp-Draft", "Stamp: Draft", ""),
    c("stamp-Final", "Stamp: Final", ""),
    c("stamp-Confidential", "Stamp: Confidential", ""),
    c("stamp-ForComment", "Stamp: For comment", ""),
    c("tool-select", "Tool: select text", "Esc"),
    c("tool-note", "Tool: note", ""),
    c("tool-text", "Tool: text box", ""),
    c("tool-rect", "Tool: rectangle", ""),
    c("tool-ellipse", "Tool: ellipse", ""),
    c("tool-line", "Tool: line", ""),
    c("tool-ink", "Tool: draw", ""),
    c("tool-redact", "Tool: redact an area", ""),
    c("tool-certify", "Tool: sign with a certificate", ""),
    c("ocr", "Recognize text (OCR)", ""),
    c("tool-edit-text", "Tool: edit text", ""),
    c("tool-add-text", "Tool: add text", ""),
    c("tool-edit-image", "Tool: edit images", ""),
    c("image-add", "Add an image", ""),
    c("image-replace", "Replace the picked image", ""),
    c("tool-link", "Tool: add or delete links", ""),
    c("bookmark-add", "Add a bookmark", "Ctrl+B"),
    c("tool-prepare", "Tool: prepare form", ""),
    c("export-word", "Export to Word", ""),
    c("export-excel", "Export to Excel", ""),
    c("export-powerpoint", "Export to PowerPoint", ""),
    c("export-odt", "Export to OpenDocument text", ""),
    c("export-png", "Export pages as PNG images", ""),
    c("export-jpeg", "Export pages as JPEG images", ""),
    c("export-text", "Export to plain text", ""),
    c("export-html", "Export to a web page", ""),
    c("export-markdown", "Export to Markdown", ""),
    c("create-pdf", "Create a PDF from files", ""),
    c("addon", "LibreOffice add-on: install or remove", ""),
    c("field-text", "Add a text field", ""),
    c("field-checkbox", "Add a checkbox", ""),
    c("field-radio", "Add radio buttons", ""),
    c("field-dropdown", "Add a dropdown", ""),
    c("field-list", "Add a list box", ""),
    c("field-button", "Add a button", ""),
    c("field-signature", "Add a signature field", ""),
    c("field-properties", "Field properties", ""),
    c("field-detect", "Find where fields go", ""),
    c("field-order-rows", "Tab order: by rows", ""),
    c("field-order-columns", "Tab order: by columns", ""),
    c("measure-distance", "Tool: measure a distance", ""),
    c("measure-perimeter", "Tool: measure a perimeter", ""),
    c("measure-area", "Tool: measure an area", ""),
    c("measure-scale", "Set the measuring scale", ""),
    c("compare", "Compare with a file", ""),
    c("split-same", "View: show this document twice", ""),
    c("split-file", "View: show a file beside this one", ""),
    c("split-close", "View: close the second pane", ""),
    c("pages-rotate", "Pages: rotate", ""),
    c(
        "pages-rotate-cw",
        "Pages: rotate the picked pages right",
        "",
    ),
    c(
        "pages-rotate-ccw",
        "Pages: rotate the picked pages left",
        "",
    ),
    c("pages-delete-some", "Pages: delete", ""),
    c("pages-move", "Pages: move", ""),
    c("pages-duplicate", "Pages: duplicate the picked pages", ""),
    c("pages-insert-blank", "Pages: insert a blank page", ""),
    c("pages-insert-file", "Pages: insert from a file", ""),
    c("pages-replace", "Pages: replace from a file", ""),
    c("pages-extract", "Pages: extract to a new PDF", ""),
    c("pages-crop", "Pages: crop", ""),
    c("page-labels", "Pages: number (page labels)", ""),
    c("split", "Split document into files", ""),
    c("combine", "Combine files into one PDF", ""),
    c("header-footer", "Add header and footer", ""),
    c("watermark", "Add watermark", ""),
    c("bates", "Add Bates numbers", ""),
    c("protect", "Protect with a password", ""),
    c("unprotect", "Remove password protection", ""),
    c("optimize", "Reduce file size", ""),
    c("batch", "Run a tool on many files", ""),
    c("update-check", "Check for updates", ""),
    c(
        "update-auto",
        "Check for updates when micropdf starts (on or off)",
        "",
    ),
    c("doc-properties", "Edit document properties", ""),
    c("sanitize", "Sanitize document", ""),
    c("redact-selection", "Redact selected text", ""),
    c("redact-search", "Search and redact", ""),
    c("redact-apply", "Apply redactions", ""),
    c("fullscreen", "Full screen", "F11"),
    c("present", "Presentation mode", "Ctrl+L"),
    c("print", "Print", "Ctrl+P"),
    c("reload", "Reload from disk", "F5"),
    c("register-pdf", "Add micropdf to Windows PDF apps", ""),
    c(
        "unregister-pdf",
        "Remove micropdf from Windows PDF apps",
        "",
    ),
];

/// Scores `candidate` against `query` as a subsequence match, case-insensitively. Higher is
/// better; None if not all query characters appear in order. Consecutive and word-start
/// matches score extra.
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    let query: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let chars: Vec<char> = candidate.to_lowercase().chars().collect();
    let mut qi = 0;
    let mut score = 0;
    let mut prev_match: Option<usize> = None;
    for (i, &ch) in chars.iter().enumerate() {
        if qi < query.len() && ch == query[qi] {
            score += 1;
            if prev_match == Some(i.wrapping_sub(1)) {
                score += 3;
            }
            if i == 0 || !chars[i - 1].is_alphanumeric() {
                score += 4;
            }
            prev_match = Some(i);
            qi += 1;
        }
    }
    (qi == query.len()).then_some(score - chars.len() as i32 / 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matching() {
        assert!(score("zin", "Zoom in").is_some());
        assert!(score("xyz", "Zoom in").is_none());
        assert!(score("fw", "Fit width") > score("fw", "Forward"));
        assert_eq!(score("", "anything"), Some(0));
    }
}
