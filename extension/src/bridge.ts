// Talks to micropdf-bridge, the native messaging host (crates/mp-bridge). A PDF goes as `begin`,
// base64 `chunk`s, each acknowledged so the browser never queues a whole file, and `end`; `open`
// opens a local PDF in the app as it is. The assistant's questions go the same way (`request`).

export const HOST = "com.micropdf.bridge";
/** Bytes per chunk; base64 makes the message a third larger. */
export const CHUNK = 768 * 1024;

/** A new file that opens in micropdf, or a local PDF to replace. */
export type Destination = { name: string } | { target: string };

type Reply = { type: string; path?: string; message?: string } & Record<string, unknown>;

/** The part of chrome.runtime.Port the exchange uses, so tests can stand in for the bridge. */
export interface PortLike {
  postMessage(message: object): void;
  onMessage: { addListener(listener: (message: Reply) => void): void };
  onDisconnect: { addListener(listener: () => void): void };
  disconnect(): void;
}

export type Connect = () => PortLike;

const connectNative: Connect = () => chrome.runtime.connectNative(HOST);

function base64(bytes: Uint8Array): string {
  let text = "";
  for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(text);
}

/** One request at a time: each message waits for the bridge's answer. */
function exchange(port: PortLike): (message: object) => Promise<Reply> {
  let pending: { resolve: (r: Reply) => void; reject: (e: Error) => void } | null = null;
  port.onMessage.addListener((reply) => {
    const p = pending;
    pending = null;
    if (reply.type === "error") p?.reject(new Error(reply.message));
    else p?.resolve(reply);
  });
  port.onDisconnect.addListener(() => {
    const p = pending;
    pending = null;
    const cause = globalThis.chrome?.runtime?.lastError?.message ?? "";
    p?.reject(
      new Error(
        /not found/i.test(cause)
          ? "micropdf for Windows is not installed, or its browser bridge is not registered"
          : cause || "the bridge closed the connection",
      ),
    );
  });
  return (message) =>
    new Promise((resolve, reject) => {
      pending = { resolve, reject };
      port.postMessage(message);
    });
}

async function talk<T>(connect: () => PortLike, use: (ask: (m: object) => Promise<Reply>) => Promise<T>): Promise<T> {
  const port = connect();
  try {
    return await use(exchange(port));
  } finally {
    port.disconnect();
  }
}

/** Sends a PDF to the bridge; resolves with where the file is now. */
export function sendPdf(bytes: Uint8Array, to: Destination, connect = connectNative): Promise<string> {
  return talk(connect, async (ask) => {
    await ask({ type: "begin", ...to });
    for (let i = 0; i < bytes.length; i += CHUNK) {
      await ask({ type: "chunk", data: base64(bytes.subarray(i, i + CHUNK)) });
    }
    return (await ask({ type: "end" })).path ?? "";
  });
}

/** One message and the bridge's answer to it. */
export function request<T>(message: object, connect = connectNative): Promise<Reply & T> {
  return talk(connect, async (ask) => (await ask(message)) as Reply & T);
}

/** Opens the local PDF at `path` in micropdf. */
export function openLocal(path: string, connect = connectNative): Promise<string> {
  return talk(connect, async (ask) => (await ask({ type: "open", path })).path ?? "");
}
