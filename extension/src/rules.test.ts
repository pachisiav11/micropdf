import { describe, expect, it } from "vitest";
import { fileName, pdfRule, pdfSource, startPage } from "./rules";

const VIEWER = "chrome-extension://phhaejfhblmccnkhnhjflbhckkanlnki/viewer.html";

describe("pdfRule", () => {
  it("redirects PDF answers to GETs, not downloads", () => {
    const rule = pdfRule(VIEWER);
    expect(rule.action.redirect?.regexSubstitution).toBe(`${VIEWER}#\\0`);
    expect(rule.condition.requestMethods).toEqual(["get"]);
    expect(rule.condition.responseHeaders?.[0].values).toEqual(["application/pdf*"]);
    expect(rule.condition.excludedResponseHeaders?.[0].header).toBe("content-disposition");
  });

  it("keeps the whole URL, query included", () => {
    const pdf = "https://example.test/files/a.pdf?token=1&x=2";
    const redirected = pdf.replace(new RegExp(pdfRule(VIEWER).condition.regexFilter!), `${VIEWER}#$&`);
    expect(pdfSource(redirected)).toBe(pdf);
  });
});

describe("pdfSource", () => {
  it("reads only web URLs", () => {
    expect(pdfSource(`${VIEWER}#https://a.test/x.pdf`)).toBe("https://a.test/x.pdf");
    expect(pdfSource(`${VIEWER}#javascript:alert(1)`)).toBeNull();
    expect(pdfSource(VIEWER)).toBeNull();
  });
});

describe("fileName", () => {
  it("takes the decoded last path part", () => {
    expect(fileName("https://a.test/docs/Q3%20report.pdf?x=1")).toBe("Q3 report.pdf");
    expect(fileName("https://a.test/")).toBe("document.pdf");
    expect(fileName("not a url")).toBe("document.pdf");
  });
});

describe("startPage", () => {
  it("reads the page open parameter", () => {
    expect(startPage("https://a.org/f.pdf#page=3")).toBe(2);
    expect(startPage("https://a.org/f.pdf#zoom=50&page=10")).toBe(9);
    expect(startPage("https://a.org/f.pdf")).toBeNull();
    expect(startPage("https://a.org/f.pdf#page=0")).toBeNull();
    expect(startPage("https://a.org/f.pdf#nameddest=intro")).toBeNull();
  });
});
