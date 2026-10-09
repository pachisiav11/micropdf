"""Writes fixtures/signer.pfx: a self-signed RSA certificate and key for signing tests, with the
password "test". It is a test key and signs nothing else. Run once; the result is committed, so
the signed fixtures made with it stay valid.

    python scripts/make_test_signer.py
"""

import datetime
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.hazmat.primitives.serialization import pkcs12
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

OUT = Path(__file__).resolve().parent.parent / "fixtures" / "signer.pfx"


def main() -> None:
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name(
        [
            x509.NameAttribute(NameOID.COMMON_NAME, "micropdf test signer"),
            x509.NameAttribute(NameOID.ORGANIZATION_NAME, "micropdf tests"),
            x509.NameAttribute(NameOID.COUNTRY_NAME, "US"),
        ]
    )
    now = datetime.datetime(2026, 1, 1, tzinfo=datetime.timezone.utc)
    cert = (
        x509.CertificateBuilder()
        .subject_name(name)
        .issuer_name(name)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now)
        .not_valid_after(now + datetime.timedelta(days=365 * 30))
        .add_extension(
            x509.KeyUsage(
                digital_signature=True,
                content_commitment=True,
                key_encipherment=False,
                data_encipherment=False,
                key_agreement=False,
                key_cert_sign=False,
                crl_sign=False,
                encipher_only=False,
                decipher_only=False,
            ),
            critical=True,
        )
        .add_extension(
            x509.ExtendedKeyUsage([ExtendedKeyUsageOID.EMAIL_PROTECTION]), critical=False
        )
        .sign(key, hashes.SHA256())
    )
    # AES and SHA-256 inside the PKCS #12; Windows reads it from Windows 10 1709 on.
    encryption = (
        serialization.PrivateFormat.PKCS12.encryption_builder()
        .kdf_rounds(2048)
        .key_cert_algorithm(pkcs12.PBES.PBESv2SHA256AndAES256CBC)
        .hmac_hash(hashes.SHA256())
        .build(b"test")
    )
    OUT.write_bytes(
        pkcs12.serialize_key_and_certificates(b"micropdf test signer", key, cert, None, encryption)
    )
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
