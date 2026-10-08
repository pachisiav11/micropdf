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

/** The PDF a viewer URL made by {@link pdfRule} is for, or null. */
export function pdfSource(viewerUrl: string): string | null {
  const hash = new URL(viewerUrl).hash.slice(1);
  if (!/^https?:\/\//i.test(hash)) return null;
  return hash;
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
