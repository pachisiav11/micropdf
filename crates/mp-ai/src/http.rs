//! HTTPS POST through WinHTTP, which brings the system's TLS, certificate store and proxy
//! settings without adding an HTTP stack to the binary.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use serde_json::Value;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_CANNOT_CONNECT, ERROR_WINHTTP_NAME_NOT_RESOLVED, ERROR_WINHTTP_TIMEOUT,
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};

use crate::Transport;

pub struct WinHttp;

struct Handle(*mut c_void);

// WinHTTP handles may be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn check(ok: bool) -> Result<(), String> {
    if ok {
        return Ok(());
    }
    Err(match unsafe { GetLastError() } {
        ERROR_WINHTTP_TIMEOUT => {
            "The API did not respond in time. Try again, or name a faster model.".into()
        }
        ERROR_WINHTTP_NAME_NOT_RESOLVED | ERROR_WINHTTP_CANNOT_CONNECT => {
            "Could not reach the API. Check the internet connection.".into()
        }
        code => format!("Could not reach the API (WinHTTP error {code})."),
    })
}

/// One session for the process: it keeps connections open between requests.
fn session() -> Result<&'static Handle, String> {
    static SESSION: OnceLock<Result<Handle, String>> = OnceLock::new();
    SESSION
        .get_or_init(|| unsafe {
            let agent = wide("micropdf");
            let handle = Handle(WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                null(),
                null(),
                0,
            ));
            check(!handle.0.is_null())?;
            // A reasoning model can stay quiet for a long time before it answers, but never this
            // long: past here the request is hung, not slow.
            WinHttpSetTimeouts(handle.0, 15_000, 15_000, 60_000, 180_000);
            Ok(handle)
        })
        .as_ref()
        .map_err(Clone::clone)
}

impl Transport for WinHttp {
    fn post(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &Value,
    ) -> Result<(u16, Value), String> {
        let rest = url
            .strip_prefix("https://")
            .ok_or("Only HTTPS addresses are supported.")?;
        let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let mut head = String::from("content-type: application/json\r\n");
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        let (host, path, verb, head) = (wide(host), wide(path), wide("POST"), wide(&head));
        let data = serde_json::to_vec(body).map_err(|e| e.to_string())?;
        let size = u32::try_from(data.len()).map_err(|_| "The request is too large to send.")?;
        let session = session()?;

        unsafe {
            let connect = Handle(WinHttpConnect(
                session.0,
                host.as_ptr(),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            ));
            check(!connect.0.is_null())?;
            let request = Handle(WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                path.as_ptr(),
                null(),
                null(),
                null(),
                WINHTTP_FLAG_SECURE,
            ));
            check(!request.0.is_null())?;
            check(
                WinHttpSendRequest(
                    request.0,
                    head.as_ptr(),
                    u32::MAX,
                    data.as_ptr().cast(),
                    size,
                    size,
                    0,
                ) != 0,
            )?;
            check(WinHttpReceiveResponse(request.0, null_mut()) != 0)?;

            let mut status = 0u32;
            let mut length = size_of::<u32>() as u32;
            check(
                WinHttpQueryHeaders(
                    request.0,
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    null(),
                    (&raw mut status).cast(),
                    &mut length,
                    null_mut(),
                ) != 0,
            )?;

            let mut reply = Vec::new();
            loop {
                let mut available = 0u32;
                check(WinHttpQueryDataAvailable(request.0, &mut available) != 0)?;
                if available == 0 {
                    break;
                }
                let start = reply.len();
                reply.resize(start + available as usize, 0);
                let mut read = 0u32;
                check(
                    WinHttpReadData(
                        request.0,
                        reply[start..].as_mut_ptr().cast(),
                        available,
                        &mut read,
                    ) != 0,
                )?;
                reply.truncate(start + read as usize);
            }
            Ok((
                status as u16,
                serde_json::from_slice(&reply).unwrap_or(Value::Null),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reaches the real API, so it runs only on request: `cargo test -p mp-ai -- --ignored`.
    #[test]
    #[ignore]
    fn a_wrong_key_reaches_anthropic_and_is_refused() {
        let body = serde_json::json!({"model": "x", "max_tokens": 1, "messages": []});
        let headers = [
            ("x-api-key", "not-a-key"),
            ("anthropic-version", "2023-06-01"),
        ];
        let (status, reply) = WinHttp
            .post("https://api.anthropic.com/v1/messages", &headers, &body)
            .unwrap();
        assert_eq!(status, 401);
        assert!(reply["error"]["message"].is_string());
    }
}
