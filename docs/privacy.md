# Privacy

micropdf, the Windows app and the browser extension, collects nothing. There is no telemetry, no
crash reporting, no account and no analytics. Your documents stay on your computer unless you
send them somewhere yourself.

## When micropdf uses the network

Only for these, and only when you ask:

| What | Where it goes | What is sent |
|---|---|---|
| The assistant, when you send a question | The provider you set up: Anthropic, OpenAI or Google AI Studio | Your question, the chat so far, and the text of the document (all of it when it fits, otherwise the pages that best match the question), with your own API key |
| Checking for updates, when you ask or at start if you turn that on | GitHub (the latest release's manifest, and the release itself if you update) | Ordinary web requests; nothing about you or your files |
| The LibreOffice add-on, when you install it | The Document Foundation's download site and its mirrors | An ordinary web request |
| Signing with a timestamp server you name | That server | A hash of the signature, as RFC 3161 requires |
| A web link in a document | Your browser, after you confirm | The link |

Text recognition uses the OCR engine built into Windows, on your computer. Office files become
PDFs through Microsoft Office or LibreOffice on your computer.

## What micropdf keeps on your computer

- `%APPDATA%\micropdf`: settings (recent files, where you were reading, colours, the library's
  folders), saved signature and initials images, the assistant's provider, model and the tokens
  used today, and each document's assistant chat, named by a hash of the document.
- `%LOCALAPPDATA%\micropdf`: the library's text of your PDFs, so they can be searched, and the
  LibreOffice add-on if you installed it.
- Windows Credential Manager: your assistant API key.

Uninstalling removes the program; delete these folders to remove the rest.

## The browser extension

The extension shows PDFs in its own viewer instead of the browser's. It reads a PDF from the
address it was opened from, with your browser's cookies, as the browser's viewer would, and it
stays in the browser unless you hand it to the desktop app. Its options are kept in your
browser's synced storage. The assistant in the extension goes through the desktop app, as above.

What each permission is for:

- **Read and change data on all websites** and **declarativeNetRequestWithHostAccess**: send a PDF
  that any site serves to the viewer instead of the browser's.
- **webNavigation**: notice PDFs opened from your computer (file:// addresses), which the rule
  above cannot catch.
- **activeTab**: the toolbar button works on the tab you click it in.
- **nativeMessaging**: talk to micropdf for Windows to open PDFs in it, save PDFs from your
  computer in place, and ask the assistant.
- **contextMenus**: "Open link in micropdf" on a link's menu.
- **storage**: keep the options.
- **debugger** (asked for the first time you use it): print the page you are on to a PDF.
- **downloads** (asked for the first time you use it): save that PDF when the desktop app is not
  installed.

## Contact

Questions and reports: <https://github.com/pachisiav11/micropdf/issues>.
