/*
 * MuPDF's pdf_pkcs7_signer and pdf_pkcs7_verifier on Windows CryptoAPI, so signing and
 * checking digital signatures need no OpenSSL. Certificates come from the user's personal
 * store (which includes smart cards and USB tokens whose drivers register with Windows) or
 * from a .pfx file.
 *
 * Signatures are detached CMS SignedData with SHA-256 and the signing-certificate-v2
 * attribute, as PAdES baseline B-B asks; with a timestamp authority's URL they also carry an
 * RFC 3161 signature timestamp (B-T).
 *
 * As in journal.c, the few MuPDF declarations needed are repeated here; they match MuPDF 1.27
 * (mupdf/fitz/context.h, mupdf/fitz/stream.h and mupdf/pdf/form.h).
 */
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>
#include <wincrypt.h>
#include <ncrypt.h>
#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct fz_context fz_context;
typedef struct fz_stream fz_stream;
typedef struct fz_image fz_image;
typedef struct pdf_annot pdf_annot;

jmp_buf *fz_push_try(fz_context *ctx);
int fz_do_try(fz_context *ctx);
int fz_do_catch(fz_context *ctx);
const char *fz_caught_message(fz_context *ctx);
__declspec(noreturn) void fz_throw(fz_context *ctx, int errcode, const char *fmt, ...);
void *fz_malloc(fz_context *ctx, size_t size);
void *fz_calloc(fz_context *ctx, size_t count, size_t size);
char *fz_strdup(fz_context *ctx, const char *s);
void fz_free(fz_context *ctx, void *p);
size_t fz_read(fz_context *ctx, fz_stream *stm, unsigned char *data, size_t len);

#define FZ_ERROR_GENERIC 1

typedef struct
{
	char *cn;
	char *o;
	char *ou;
	char *email;
	char *c;
} pdf_pkcs7_distinguished_name;

typedef enum
{
	PDF_SIGNATURE_ERROR_OKAY,
	PDF_SIGNATURE_ERROR_NO_SIGNATURES,
	PDF_SIGNATURE_ERROR_NO_CERTIFICATE,
	PDF_SIGNATURE_ERROR_DIGEST_FAILURE,
	PDF_SIGNATURE_ERROR_SELF_SIGNED,
	PDF_SIGNATURE_ERROR_SELF_SIGNED_IN_CHAIN,
	PDF_SIGNATURE_ERROR_NOT_TRUSTED,
	PDF_SIGNATURE_ERROR_NOT_SIGNED,
	PDF_SIGNATURE_ERROR_UNKNOWN,
} pdf_signature_error;

typedef struct pdf_pkcs7_signer pdf_pkcs7_signer;
struct pdf_pkcs7_signer
{
	pdf_pkcs7_signer *(*keep)(fz_context *ctx, pdf_pkcs7_signer *signer);
	void (*drop)(fz_context *ctx, pdf_pkcs7_signer *signer);
	pdf_pkcs7_distinguished_name *(*get_signing_name)(fz_context *ctx, pdf_pkcs7_signer *signer);
	size_t (*max_digest_size)(fz_context *ctx, pdf_pkcs7_signer *signer);
	int (*create_digest)(fz_context *ctx, pdf_pkcs7_signer *signer, fz_stream *in, unsigned char *digest, size_t digest_len);
};

typedef struct pdf_pkcs7_verifier pdf_pkcs7_verifier;
struct pdf_pkcs7_verifier
{
	void (*drop)(fz_context *ctx, pdf_pkcs7_verifier *verifier);
	pdf_signature_error (*check_certificate)(fz_context *ctx, pdf_pkcs7_verifier *verifier, unsigned char *signature, size_t len);
	pdf_signature_error (*check_digest)(fz_context *ctx, pdf_pkcs7_verifier *verifier, fz_stream *in, unsigned char *signature, size_t len);
	pdf_pkcs7_distinguished_name *(*get_signatory)(fz_context *ctx, pdf_pkcs7_verifier *verifier, unsigned char *signature, size_t len);
};

void pdf_sign_signature(fz_context *ctx, pdf_annot *widget, pdf_pkcs7_signer *signer, int appearance_flags, fz_image *graphic, const char *reason, const char *location);
pdf_signature_error pdf_check_widget_digest(fz_context *ctx, pdf_pkcs7_verifier *verifier, pdf_annot *widget);
pdf_signature_error pdf_check_widget_certificate(fz_context *ctx, pdf_pkcs7_verifier *verifier, pdf_annot *widget);
pdf_pkcs7_distinguished_name *pdf_signature_get_widget_signatory(fz_context *ctx, pdf_pkcs7_verifier *verifier, pdf_annot *widget);
void pdf_signature_drop_distinguished_name(fz_context *ctx, pdf_pkcs7_distinguished_name *name);
int pdf_incremental_change_since_signing_widget(fz_context *ctx, pdf_annot *widget);

#define TRY(ctx) if (!setjmp(*fz_push_try(ctx))) if (fz_do_try(ctx)) do
#define CATCH(ctx) while (0); if (fz_do_catch(ctx))

#define ENCODING (X509_ASN_ENCODING | PKCS_7_ASN_ENCODING)
#define CHUNK 65536
/* id-aa-signingCertificateV2 and id-aa-signatureTimeStampToken. */
#define OID_SIGNING_CERTIFICATE_V2 "1.2.840.113549.1.9.16.2.47"
#define OID_SIGNATURE_TIMESTAMP "1.2.840.113549.1.9.16.2.14"

typedef struct
{
	pdf_pkcs7_signer base;
	int refs;
	HCERTSTORE store;
	PCCERT_CONTEXT cert;
	/* The certificates to embed: the signer's chain, without the root when it is known. */
	PCCERT_CONTEXT chain[8];
	DWORD chain_len;
	wchar_t *tsa;
} signer;

/* ---------------------------------------------------------------- names */

static char *utf8(fz_context *ctx, const wchar_t *w)
{
	int n = WideCharToMultiByte(CP_UTF8, 0, w, -1, NULL, 0, NULL, NULL);
	char *s;
	if (n <= 1)
		return NULL;
	s = fz_malloc(ctx, n);
	WideCharToMultiByte(CP_UTF8, 0, w, -1, s, n, NULL, NULL);
	return s;
}

static char *cert_name(fz_context *ctx, PCCERT_CONTEXT cert, DWORD type, const char *oid)
{
	wchar_t buf[512];
	DWORD n = CertGetNameStringW(cert, type, 0, (void *)oid, buf, 512);
	return n > 1 ? utf8(ctx, buf) : NULL;
}

static pdf_pkcs7_distinguished_name *names(fz_context *ctx, PCCERT_CONTEXT cert)
{
	pdf_pkcs7_distinguished_name *dn = fz_calloc(ctx, 1, sizeof *dn);
	dn->cn = cert_name(ctx, cert, CERT_NAME_ATTR_TYPE, szOID_COMMON_NAME);
	if (!dn->cn)
		dn->cn = cert_name(ctx, cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, NULL);
	dn->o = cert_name(ctx, cert, CERT_NAME_ATTR_TYPE, szOID_ORGANIZATION_NAME);
	dn->ou = cert_name(ctx, cert, CERT_NAME_ATTR_TYPE, szOID_ORGANIZATIONAL_UNIT_NAME);
	dn->email = cert_name(ctx, cert, CERT_NAME_EMAIL_TYPE, NULL);
	dn->c = cert_name(ctx, cert, CERT_NAME_ATTR_TYPE, szOID_COUNTRY_NAME);
	return dn;
}

/* ---------------------------------------------------------------- signing */

static pdf_pkcs7_signer *keep_signer(fz_context *ctx, pdf_pkcs7_signer *base)
{
	((signer *)base)->refs++;
	return base;
}

static void drop_signer(fz_context *ctx, pdf_pkcs7_signer *base)
{
	signer *s = (signer *)base;
	DWORD i;
	if (!s || --s->refs > 0)
		return;
	for (i = 0; i < s->chain_len; i++)
		CertFreeCertificateContext(s->chain[i]);
	if (s->cert)
		CertFreeCertificateContext(s->cert);
	if (s->store)
		CertCloseStore(s->store, 0);
	free(s->tsa);
	free(s);
}

static pdf_pkcs7_distinguished_name *signing_name(fz_context *ctx, pdf_pkcs7_signer *base)
{
	return names(ctx, ((signer *)base)->cert);
}

static size_t max_size(fz_context *ctx, pdf_pkcs7_signer *base)
{
	signer *s = (signer *)base;
	size_t size = 4096; /* signature value, attributes and structure, with room to spare */
	DWORD i;
	for (i = 0; i < s->chain_len; i++)
		size += s->chain[i]->cbCertEncoded;
	if (s->tsa)
		size += 16384; /* a timestamp token carries the authority's own certificates */
	return size;
}

/* The DER of SigningCertificateV2 naming the certificate by its SHA-256 hash (the default
 * algorithm, so it is left out): SEQUENCE { SEQUENCE { SEQUENCE { OCTET STRING hash } } }. */
static BOOL signing_certificate(PCCERT_CONTEXT cert, BYTE out[40])
{
	static const BYTE head[8] = { 0x30, 0x26, 0x30, 0x24, 0x30, 0x22, 0x04, 0x20 };
	DWORD n = 32;
	memcpy(out, head, 8);
	return CryptHashCertificate2(L"SHA256", 0, NULL, cert->pbCertEncoded, cert->cbCertEncoded, out + 8, &n) && n == 32;
}

/* Adds an RFC 3161 timestamp of the signature value from the authority at `tsa` to the
 * encoded message; returns the new encoding in *out (LocalFree it), or FALSE. */
static BOOL add_timestamp(const wchar_t *tsa, BYTE *encoded, DWORD len, BYTE **out, DWORD *out_len)
{
	HCRYPTMSG msg = NULL;
	BYTE *value = NULL, *attr = NULL;
	DWORD value_len = 0, attr_len = 0;
	PCRYPT_TIMESTAMP_CONTEXT stamp = NULL;
	CRYPT_ATTR_BLOB token;
	CRYPT_ATTRIBUTE attribute;
	CMSG_CTRL_ADD_SIGNER_UNAUTH_ATTR_PARA add;
	BOOL ok = FALSE;

	*out = NULL;
	msg = CryptMsgOpenToDecode(ENCODING, CMSG_DETACHED_FLAG, 0, 0, NULL, NULL);
	if (!msg || !CryptMsgUpdate(msg, encoded, len, TRUE))
		goto done;
	if (!CryptMsgGetParam(msg, CMSG_ENCRYPTED_DIGEST, 0, NULL, &value_len))
		goto done;
	value = LocalAlloc(LMEM_FIXED, value_len);
	if (!value || !CryptMsgGetParam(msg, CMSG_ENCRYPTED_DIGEST, 0, value, &value_len))
		goto done;
	if (!CryptRetrieveTimeStamp(tsa, TIMESTAMP_NO_AUTH_RETRIEVAL, 30000, szOID_NIST_sha256, NULL, value, value_len, &stamp, NULL, NULL))
		goto done;
	token.cbData = stamp->cbEncoded;
	token.pbData = stamp->pbEncoded;
	attribute.pszObjId = OID_SIGNATURE_TIMESTAMP;
	attribute.cValue = 1;
	attribute.rgValue = &token;
	if (!CryptEncodeObjectEx(ENCODING, PKCS_ATTRIBUTE, &attribute, CRYPT_ENCODE_ALLOC_FLAG, NULL, &attr, &attr_len))
		goto done;
	memset(&add, 0, sizeof add);
	add.cbSize = sizeof add;
	add.dwSignerIndex = 0;
	add.blob.cbData = attr_len;
	add.blob.pbData = attr;
	if (!CryptMsgControl(msg, 0, CMSG_CTRL_ADD_SIGNER_UNAUTH_ATTR, &add))
		goto done;
	if (!CryptMsgGetParam(msg, CMSG_ENCODED_MESSAGE, 0, NULL, out_len))
		goto done;
	*out = LocalAlloc(LMEM_FIXED, *out_len);
	ok = *out && CryptMsgGetParam(msg, CMSG_ENCODED_MESSAGE, 0, *out, out_len);
done:
	if (!ok && *out)
	{
		LocalFree(*out);
		*out = NULL;
	}
	if (attr)
		LocalFree(attr);
	if (stamp)
		CryptMemFree(stamp);
	if (value)
		LocalFree(value);
	if (msg)
		CryptMsgClose(msg);
	return ok;
}

#define FAIL(why) do { problem = why; code = GetLastError(); goto done; } while (0)

static int create_digest(fz_context *ctx, pdf_pkcs7_signer *base, fz_stream *in, unsigned char *digest, size_t digest_len)
{
	signer *s = (signer *)base;
	HCRYPTPROV_OR_NCRYPT_KEY_HANDLE key = 0;
	DWORD spec = 0, i, len = 0;
	BOOL free_key = FALSE;
	HCRYPTMSG msg = NULL;
	CMSG_SIGNER_ENCODE_INFO info;
	CMSG_SIGNED_ENCODE_INFO signed_info;
	CERT_BLOB certs[8];
	BYTE certificate[40];
	CRYPT_ATTR_BLOB certificate_blob;
	CRYPT_ATTRIBUTE attribute;
	BYTE *chunk = NULL, *encoded = NULL, *stamped = NULL;
	DWORD stamped_len = 0;
	const char *problem = NULL;
	DWORD code = 0;
	size_t n;

	/* The cache flag finds the key of a .pfx imported without saving it, which is held only in
	 * the certificate's properties; the certificate then owns the handle. */
	if (!CryptAcquireCertificatePrivateKey(s->cert, CRYPT_ACQUIRE_CACHE_FLAG | CRYPT_ACQUIRE_PREFER_NCRYPT_KEY_FLAG, NULL, &key, &spec, &free_key))
		FAIL("the certificate's private key is not available");
	if (!signing_certificate(s->cert, certificate))
		FAIL("could not hash the certificate");
	certificate_blob.cbData = 40;
	certificate_blob.pbData = certificate;
	attribute.pszObjId = OID_SIGNING_CERTIFICATE_V2;
	attribute.cValue = 1;
	attribute.rgValue = &certificate_blob;

	memset(&info, 0, sizeof info);
	info.cbSize = sizeof info;
	info.pCertInfo = s->cert->pCertInfo;
	info.hCryptProv = key;
	info.dwKeySpec = spec;
	info.HashAlgorithm.pszObjId = szOID_NIST_sha256;
	/* With signed attributes present, CryptoAPI adds content-type and message-digest. */
	info.cAuthAttr = 1;
	info.rgAuthAttr = &attribute;

	for (i = 0; i < s->chain_len; i++)
	{
		certs[i].cbData = s->chain[i]->cbCertEncoded;
		certs[i].pbData = s->chain[i]->pbCertEncoded;
	}
	memset(&signed_info, 0, sizeof signed_info);
	signed_info.cbSize = sizeof signed_info;
	signed_info.cSigners = 1;
	signed_info.rgSigners = &info;
	signed_info.cCertEncoded = s->chain_len;
	signed_info.rgCertEncoded = certs;

	msg = CryptMsgOpenToEncode(ENCODING, CMSG_DETACHED_FLAG, CMSG_SIGNED, &signed_info, NULL, NULL);
	if (!msg)
		FAIL("could not start the signature");
	chunk = malloc(CHUNK);
	if (!chunk)
		FAIL("out of memory");
	/* fz_read throws on a read error; the document's own file is being read, so it has been
	 * read before and errors here are not expected. */
	while ((n = fz_read(ctx, in, chunk, CHUNK)) > 0)
	{
		if (!CryptMsgUpdate(msg, chunk, (DWORD)n, FALSE))
			FAIL("could not hash the document");
	}
	if (!CryptMsgUpdate(msg, NULL, 0, TRUE))
		FAIL("could not sign; the key may need its PIN, or signing was cancelled");
	if (!CryptMsgGetParam(msg, CMSG_CONTENT_PARAM, 0, NULL, &len) || !(encoded = malloc(len)) || !CryptMsgGetParam(msg, CMSG_CONTENT_PARAM, 0, encoded, &len))
		FAIL("could not encode the signature");
	if (s->tsa)
	{
		if (!add_timestamp(s->tsa, encoded, len, &stamped, &stamped_len))
			FAIL("the timestamp authority did not answer");
	}
done:
	if (!problem)
	{
		BYTE *out = stamped ? stamped : encoded;
		DWORD out_len = stamped ? stamped_len : len;
		if (out_len > digest_len)
			problem = "the signature is larger than the space kept for it";
		else
			memcpy(digest, out, out_len);
		len = out_len;
	}
	if (stamped)
		LocalFree(stamped);
	free(encoded);
	free(chunk);
	if (msg)
		CryptMsgClose(msg);
	if (free_key && key)
	{
		if (spec == CERT_NCRYPT_KEY_SPEC)
			NCryptFreeObject(key);
		else
			CryptReleaseContext(key, 0);
	}
	if (problem && code)
		fz_throw(ctx, FZ_ERROR_GENERIC, "%s (error %x)", problem, (unsigned int)code);
	if (problem)
		fz_throw(ctx, FZ_ERROR_GENERIC, "%s", problem);
	return (int)len;
}

static signer *new_signer(HCERTSTORE store, PCCERT_CONTEXT cert, const wchar_t *tsa)
{
	signer *s = calloc(1, sizeof *s);
	CERT_CHAIN_PARA para;
	PCCERT_CHAIN_CONTEXT chain = NULL;
	if (!s)
	{
		CertFreeCertificateContext(cert);
		CertCloseStore(store, 0);
		return NULL;
	}
	s->base.keep = keep_signer;
	s->base.drop = drop_signer;
	s->base.get_signing_name = signing_name;
	s->base.max_digest_size = max_size;
	s->base.create_digest = create_digest;
	s->refs = 1;
	s->store = store;
	s->cert = cert;
	if (tsa && *tsa)
		s->tsa = _wcsdup(tsa);

	memset(&para, 0, sizeof para);
	para.cbSize = sizeof para;
	if (CertGetCertificateChain(NULL, cert, NULL, store, &para, 0, NULL, &chain) && chain->cChain > 0)
	{
		PCERT_SIMPLE_CHAIN simple = chain->rgpChain[0];
		DWORD i, count = simple->cElement;
		/* A trusted root is in every verifier's store already. */
		if (count > 1 && !(chain->TrustStatus.dwErrorStatus & CERT_TRUST_IS_UNTRUSTED_ROOT))
			count--;
		for (i = 0; i < count && i < 8; i++)
			s->chain[s->chain_len++] = CertDuplicateCertificateContext(simple->rgpElement[i]->pCertContext);
	}
	if (chain)
		CertFreeCertificateChain(chain);
	if (s->chain_len == 0)
		s->chain[s->chain_len++] = CertDuplicateCertificateContext(cert);
	return s;
}

/* A signer for the certificate in the user's personal store whose SHA-1 thumbprint is `hash`. */
int mp_signer_from_store(const unsigned char hash[20], const wchar_t *tsa, pdf_pkcs7_signer **out, const char **err)
{
	HCERTSTORE store = CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0, CERT_SYSTEM_STORE_CURRENT_USER | CERT_STORE_READONLY_FLAG, L"MY");
	CRYPT_HASH_BLOB blob;
	PCCERT_CONTEXT cert;
	*out = NULL;
	if (!store)
	{
		*err = "could not open the certificate store";
		return -1;
	}
	blob.cbData = 20;
	blob.pbData = (BYTE *)hash;
	cert = CertFindCertificateInStore(store, ENCODING, 0, CERT_FIND_SHA1_HASH, &blob, NULL);
	if (!cert)
	{
		CertCloseStore(store, 0);
		*err = "the certificate is no longer in the store";
		return -1;
	}
	*out = (pdf_pkcs7_signer *)new_signer(store, cert, tsa);
	if (!*out)
	{
		*err = "out of memory";
		return -1;
	}
	return 0;
}

/* A signer for the certificate with a private key in a .pfx (PKCS #12) file's bytes. The
 * key stays in memory and is not added to the user's keys. */
int mp_signer_from_pfx(const unsigned char *data, size_t len, const wchar_t *password, const wchar_t *tsa, pdf_pkcs7_signer **out, const char **err)
{
	CRYPT_DATA_BLOB blob;
	HCERTSTORE store;
	PCCERT_CONTEXT cert;
	*out = NULL;
	blob.cbData = (DWORD)len;
	blob.pbData = (BYTE *)data;
	if (!PFXIsPFXBlob(&blob))
	{
		*err = "the file is not a .pfx or .p12 certificate";
		return -1;
	}
	store = PFXImportCertStore(&blob, password, PKCS12_NO_PERSIST_KEY | PKCS12_ALWAYS_CNG_KSP);
	if (!store)
	{
		*err = GetLastError() == ERROR_INVALID_PASSWORD ? "the password is wrong" : "could not read the certificate file";
		return -1;
	}
	cert = CertFindCertificateInStore(store, ENCODING, 0, CERT_FIND_HAS_PRIVATE_KEY, NULL, NULL);
	if (!cert)
	{
		CertCloseStore(store, 0);
		*err = "the file holds no certificate with a private key";
		return -1;
	}
	*out = (pdf_pkcs7_signer *)new_signer(store, cert, tsa);
	if (!*out)
	{
		*err = "out of memory";
		return -1;
	}
	return 0;
}

/* Calls `each` for every certificate in the user's personal store that has a private key and
 * may sign: its SHA-1 thumbprint, subject, issuer and expiry date (YYYY-MM-DD), in UTF-8. */
void mp_list_certificates(void (*each)(void *arg, const unsigned char *hash, const char *name, const char *issuer, const char *expires), void *arg)
{
	HCERTSTORE store = CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0, CERT_SYSTEM_STORE_CURRENT_USER | CERT_STORE_READONLY_FLAG, L"MY");
	PCCERT_CONTEXT cert = NULL;
	if (!store)
		return;
	while ((cert = CertFindCertificateInStore(store, ENCODING, 0, CERT_FIND_HAS_PRIVATE_KEY, NULL, cert)) != NULL)
	{
		BYTE hash[20];
		DWORD hash_len = 20;
		wchar_t wname[512], wissuer[512];
		char name[1024], issuer[1024], expires[16];
		SYSTEMTIME t;
		BYTE usage = 0;
		if (!CertGetCertificateContextProperty(cert, CERT_HASH_PROP_ID, hash, &hash_len) || hash_len != 20)
			continue;
		/* A key usage that leaves out digital signatures rules the certificate out. */
		if (CertGetIntendedKeyUsage(ENCODING, cert->pCertInfo, &usage, 1) && !(usage & (CERT_DIGITAL_SIGNATURE_KEY_USAGE | CERT_NON_REPUDIATION_KEY_USAGE)))
			continue;
		CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, NULL, wname, 512);
		CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_NAME_ISSUER_FLAG, NULL, wissuer, 512);
		WideCharToMultiByte(CP_UTF8, 0, wname, -1, name, sizeof name, NULL, NULL);
		WideCharToMultiByte(CP_UTF8, 0, wissuer, -1, issuer, sizeof issuer, NULL, NULL);
		expires[0] = 0;
		if (FileTimeToSystemTime(&cert->pCertInfo->NotAfter, &t))
			snprintf(expires, sizeof expires, "%04d-%02d-%02d", t.wYear, t.wMonth, t.wDay);
		each(arg, hash, name, issuer, expires);
	}
	CertCloseStore(store, 0);
}

void mp_signer_drop(pdf_pkcs7_signer *s)
{
	if (s)
		drop_signer(NULL, s);
}

/* The signer's name as MuPDF shows it, in `name` (UTF-8, at most `size` bytes with the NUL). */
void mp_signer_name(pdf_pkcs7_signer *base, char *name, size_t size)
{
	wchar_t buf[512];
	signer *s = (signer *)base;
	name[0] = 0;
	if (CertGetNameStringW(s->cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, NULL, buf, 512) > 1)
		WideCharToMultiByte(CP_UTF8, 0, buf, -1, name, (int)size, NULL, NULL);
}

int mp_pdf_sign_signature(fz_context *ctx, pdf_annot *widget, pdf_pkcs7_signer *signer, int flags, const char *reason, const char *location, const char **err)
{
	TRY(ctx) { pdf_sign_signature(ctx, widget, signer, flags, NULL, reason, location); }
	CATCH(ctx) { *err = fz_caught_message(ctx); return -1; }
	return 0;
}

/* ---------------------------------------------------------------- checking */

/* The signature's length without the zeros that pad it out to its reserved space. */
static DWORD der_length(const unsigned char *sig, size_t len)
{
	size_t n, i, value = 0;
	if (len < 2 || sig[0] != 0x30)
		return (DWORD)len;
	if (sig[1] < 0x80)
		return (DWORD)(sig[1] + 2 <= len ? sig[1] + 2 : len);
	n = sig[1] & 0x7f;
	if (n == 0 || n > 4 || 2 + n > len)
		return (DWORD)len;
	for (i = 0; i < n; i++)
		value = (value << 8) | sig[2 + i];
	return (DWORD)(2 + n + value <= len ? 2 + n + value : len);
}

/* Opens the signature for checking, and finds the signer's certificate in it. */
static HCRYPTMSG open_signature(const unsigned char *sig, size_t len, HCERTSTORE *store, PCCERT_CONTEXT *cert)
{
	HCRYPTMSG msg = CryptMsgOpenToDecode(ENCODING, CMSG_DETACHED_FLAG, 0, 0, NULL, NULL);
	PCERT_INFO info = NULL;
	DWORD size = 0;
	*store = NULL;
	*cert = NULL;
	if (!msg)
		return NULL;
	if (!CryptMsgUpdate(msg, sig, der_length(sig, len), TRUE))
		goto fail;
	*store = CertOpenStore(CERT_STORE_PROV_MSG, ENCODING, 0, 0, msg);
	if (!*store || !CryptMsgGetParam(msg, CMSG_SIGNER_CERT_INFO_PARAM, 0, NULL, &size))
		goto fail;
	info = malloc(size);
	if (!info || !CryptMsgGetParam(msg, CMSG_SIGNER_CERT_INFO_PARAM, 0, info, &size))
		goto fail;
	*cert = CertGetSubjectCertificateFromStore(*store, ENCODING, info);
	free(info);
	if (!*cert)
		goto fail;
	return msg;
fail:
	free(info);
	if (*store)
		CertCloseStore(*store, 0);
	*store = NULL;
	CryptMsgClose(msg);
	return NULL;
}

static void close_signature(HCRYPTMSG msg, HCERTSTORE store, PCCERT_CONTEXT cert)
{
	if (cert)
		CertFreeCertificateContext(cert);
	if (store)
		CertCloseStore(store, 0);
	if (msg)
		CryptMsgClose(msg);
}

static void drop_verifier(fz_context *ctx, pdf_pkcs7_verifier *verifier)
{
	free(verifier);
}

static pdf_signature_error check_digest(fz_context *ctx, pdf_pkcs7_verifier *verifier, fz_stream *in, unsigned char *sig, size_t len)
{
	HCERTSTORE store;
	PCCERT_CONTEXT cert;
	HCRYPTMSG msg = open_signature(sig, len, &store, &cert);
	pdf_signature_error result = PDF_SIGNATURE_ERROR_DIGEST_FAILURE;
	unsigned char *chunk = malloc(CHUNK);
	size_t n;
	if (!msg)
	{
		free(chunk);
		return PDF_SIGNATURE_ERROR_NO_CERTIFICATE;
	}
	if (chunk)
	{
		BOOL ok = TRUE;
		while (ok && (n = fz_read(ctx, in, chunk, CHUNK)) > 0)
			ok = CryptMsgUpdate(msg, chunk, (DWORD)n, FALSE);
		if (ok && CryptMsgUpdate(msg, NULL, 0, TRUE) && CryptMsgControl(msg, 0, CMSG_CTRL_VERIFY_SIGNATURE, cert->pCertInfo))
			result = PDF_SIGNATURE_ERROR_OKAY;
	}
	free(chunk);
	close_signature(msg, store, cert);
	return result;
}

static pdf_signature_error check_certificate(fz_context *ctx, pdf_pkcs7_verifier *verifier, unsigned char *sig, size_t len)
{
	HCERTSTORE store;
	PCCERT_CONTEXT cert;
	HCRYPTMSG msg = open_signature(sig, len, &store, &cert);
	CERT_CHAIN_PARA para;
	PCCERT_CHAIN_CONTEXT chain = NULL;
	pdf_signature_error result = PDF_SIGNATURE_ERROR_NOT_TRUSTED;
	if (!msg)
		return PDF_SIGNATURE_ERROR_NO_CERTIFICATE;
	memset(&para, 0, sizeof para);
	para.cbSize = sizeof para;
	if (CertGetCertificateChain(NULL, cert, NULL, store, &para, 0, NULL, &chain))
	{
		DWORD status = chain->TrustStatus.dwErrorStatus;
		if (status == CERT_TRUST_NO_ERROR)
			result = PDF_SIGNATURE_ERROR_OKAY;
		else if (status & CERT_TRUST_IS_UNTRUSTED_ROOT)
			result = chain->cChain > 0 && chain->rgpChain[0]->cElement == 1 ? PDF_SIGNATURE_ERROR_SELF_SIGNED : PDF_SIGNATURE_ERROR_SELF_SIGNED_IN_CHAIN;
		CertFreeCertificateChain(chain);
	}
	close_signature(msg, store, cert);
	return result;
}

static pdf_pkcs7_distinguished_name *get_signatory(fz_context *ctx, pdf_pkcs7_verifier *verifier, unsigned char *sig, size_t len)
{
	HCERTSTORE store;
	PCCERT_CONTEXT cert;
	HCRYPTMSG msg = open_signature(sig, len, &store, &cert);
	pdf_pkcs7_distinguished_name *dn = NULL;
	if (msg)
		dn = names(ctx, cert);
	close_signature(msg, store, cert);
	return dn;
}

typedef struct
{
	int digest;
	int certificate;
	int changed;
	char signer[256];
} mp_signature_check;

/* Checks a signed signature widget: whether the signed bytes are unchanged, whether Windows
 * trusts the certificate, and whether the file was changed after signing. */
int mp_pdf_check_signature(fz_context *ctx, pdf_annot *widget, mp_signature_check *check, const char **err)
{
	pdf_pkcs7_verifier *v = calloc(1, sizeof *v);
	pdf_pkcs7_distinguished_name *dn = NULL;
	int failed = 0;
	if (!v)
	{
		*err = "out of memory";
		return -1;
	}
	v->drop = drop_verifier;
	v->check_certificate = check_certificate;
	v->check_digest = check_digest;
	v->get_signatory = get_signatory;
	memset(check, 0, sizeof *check);
	TRY(ctx)
	{
		check->digest = pdf_check_widget_digest(ctx, v, widget);
		check->certificate = pdf_check_widget_certificate(ctx, v, widget);
		check->changed = pdf_incremental_change_since_signing_widget(ctx, widget);
		dn = pdf_signature_get_widget_signatory(ctx, v, widget);
		if (dn && dn->cn)
			strncpy(check->signer, dn->cn, sizeof check->signer - 1);
	}
	CATCH(ctx)
	{
		*err = fz_caught_message(ctx);
		failed = 1;
	}
	if (dn)
		pdf_signature_drop_distinguished_name(ctx, dn);
	free(v);
	return failed ? -1 : 0;
}
