// The service worker: keeps the rule that opens web PDFs in the viewer.

import { PDF_RULE_ID, pdfRule } from "./rules";

async function installRules(): Promise<void> {
  await chrome.declarativeNetRequest.updateDynamicRules({
    removeRuleIds: [PDF_RULE_ID],
    addRules: [pdfRule(chrome.runtime.getURL("viewer.html"))],
  });
}

chrome.runtime.onInstalled.addListener(() => void installRules());
chrome.runtime.onStartup.addListener(() => void installRules());
