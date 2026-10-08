// Redirecting PDF responses to the viewer, as a declarativeNetRequest rule.

export const PDF_RULE_ID = 1;

/**
 * Sends top-level and frame GETs that answer with a PDF to `viewer`, with the PDF's URL after
 * the `#`, unless the server asks for a download. POSTed PDFs stay with the browser's own viewer:
 * they cannot be fetched again.
 */
export function pdfRule(viewer: string): chrome.declarativeNetRequest.Rule {
  return {
    id: PDF_RULE_ID,
    priority: 1,
    action: {
      type: "redirect" as chrome.declarativeNetRequest.RuleActionType,
      redirect: { regexSubstitution: `${viewer}#\\0` },
    },
    condition: {
      regexFilter: "^https?://.*",
      resourceTypes: ["main_frame", "sub_frame"] as chrome.declarativeNetRequest.ResourceType[],
      requestMethods: ["get"] as chrome.declarativeNetRequest.RequestMethod[],
      responseHeaders: [{ header: "content-type", values: ["application/pdf*"] }],
      excludedResponseHeaders: [{ header: "content-disposition", values: ["attachment*"] }],
    },
  };
}

/** The PDF a viewer URL is for, or null: a web PDF ({@link pdfRule}) or a file on this computer
 * (the service worker sends file:// PDFs to the viewer). */
export function pdfSource(viewerUrl: string): string | null {
  const hash = new URL(viewerUrl).hash.slice(1);
  if (!/^(https?|file):\/\//i.test(hash)) return null;
  return hash;
}

/** The Windows path of a file:// URL, as C:\dir.pdf or \server\share.pdf; null for others. */
export function localPath(url: string): string | null {
  if (!/^file:/i.test(url)) return null;
  const u = new URL(url);
  const path = decodeURIComponent(u.pathname).replace(/^\/([A-Za-z]:)/, "$1").replaceAll("/", "\\");
  return u.host ? `\\\\${u.host}${path}` : path;
}

/** Whether the path of `url` ends in .pdf. */
export function looksLikePdf(url: string): boolean {
  try {
    return /\.pdf$/i.test(new URL(url).pathname);
  } catch {
    return false;
  }
}

/** A name for the PDF at `url`: the last part of its path, decoded. */
export function fileName(url: string): string {
  try {
    const last = new URL(url).pathname.split("/").filter(Boolean).pop() ?? "";
    return decodeURIComponent(last) || "document.pdf";
  } catch {
    return "document.pdf";
  }
}

/** The page (from 0) that the PDF's own fragment asks for, as in `file.pdf#page=3`, or null. */
export function startPage(pdfUrl: string): number | null {
  const fragment = pdfUrl.split("#")[1] ?? "";
  const page = new URLSearchParams(fragment.replaceAll("&amp;", "&")).get("page");
  const n = page === null ? NaN : Number.parseInt(page, 10);
  return Number.isFinite(n) && n >= 1 ? n - 1 : null;
}
