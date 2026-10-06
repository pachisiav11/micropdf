# Security policy

micropdf parses untrusted PDF files. Crashes, hangs, memory corruption, or anything that lets a
PDF run code, read or write files, or reach the network without the user asking are security bugs.

## Reporting

Please report privately through **Security → Report a vulnerability** on this repository rather
than opening a public issue. Attach the PDF that triggers the problem if you can share it, or
describe how it was made if you cannot.

Bugs inside MuPDF itself are also reportable upstream to Artifex; we will coordinate and update
the bundled MuPDF version.

## Supported versions

Only the latest release receives fixes until 1.0.
