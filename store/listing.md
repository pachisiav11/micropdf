# Store listing

The texts for the Chrome Web Store and Edge Add-ons. Both stores take the same zip of
`extension/dist` (see RELEASING.md).

## Name

micropdf

## Short description

(Chrome: at most 132 characters.)

View, comment on and fill in PDFs in a fast viewer, and open them in micropdf for Windows.

## Description

micropdf shows the PDFs you open in your browser in its own viewer, built on the MuPDF engine.

- Read: thumbnails, bookmarks, search, zoom and rotation, in a light or dark theme.
- Comment: highlight, underline and strike out text, add notes, draw, and type on the page.
- Fill in forms, including simple calculated fields, and save a copy with your changes.
- Open in micropdf for Windows, the desktop app, from the viewer, the toolbar button or a link's
  menu, for editing, signing, redaction, OCR, conversion and the rest of what it does.
- Ask the assistant about the document through the desktop app, with your own API key.
- Print any web page to a PDF from the toolbar button.

No account, no tracking, no data collected. The viewer works on its own; the desktop app, free
and open source like the extension, adds the rest:
https://github.com/pachisiav11/micropdf

## Category

Productivity (Chrome); Productivity (Edge).

## Single purpose

View and work with PDF documents, in the browser and in the micropdf desktop app.

## Permission justifications

- **Host permissions (all sites) and declarativeNetRequestWithHostAccess**: PDFs come from any
  site; a rule sends each PDF response to the extension's viewer instead of the browser's.
- **webNavigation**: catches PDFs opened from the computer (file:// addresses), which the rule
  cannot redirect.
- **activeTab**: the toolbar button works on the tab it is clicked in.
- **nativeMessaging**: hands PDFs to the micropdf desktop app, saves local PDFs in place, and
  asks the desktop app's assistant.
- **contextMenus**: adds "Open link in micropdf" to links.
- **storage**: keeps the extension's options.
- **debugger** (optional, asked on first use): prints the current page to a PDF with
  `Page.printToPDF`, when the user clicks the toolbar button on a page that is not a PDF.
- **downloads** (optional, asked on first use): saves that PDF when the desktop app is not
  installed.

No remote code: everything the extension runs is in the package.

## Data use

The extension collects no user data. Privacy policy:
https://github.com/pachisiav11/micropdf/blob/main/docs/privacy.md

## Images to make before submitting

- Icon: `extension/public/icons/128.png`.
- Screenshots, 1280 x 800: the viewer with a document and its thumbnails; comments on a page; a
  form being filled in; the options page.
- Small promotional tile (Chrome), 440 x 280: the icon and the name on the brand colour.
