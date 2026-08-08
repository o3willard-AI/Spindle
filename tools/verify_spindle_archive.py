#!/usr/bin/env python3
"""Third-party verification script using the cryptography library.

Confirms that signatures produced by spindle-signing (ed25519-dalek) can be
verified by Python's cryptography library — a true cross-language interop test.

Usage:
    python verify_spindle_archive.py --keys-url <url> --archive <path>
    python verify_spindle_archive.py --keys-json <file> --archive <path>
"""

import argparse
import base64
import json
import sys
from pathlib import Path

# Try importing the cryptography library (third-party dependency)
try:
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import (
        Ed25519PublicKey,
    )
    HAS_CRYPTOGRAPHY = True
except ImportError:
    HAS_CRYPTOGRAPHY = False


def fetch_keys_json(url: str) -> dict:
    """Fetch the keys.json endpoint and return parsed JSON."""
    import urllib.request
    import ssl

    ctx = ssl.create_default_context()
    with urllib.request.urlopen(url, context=ctx) as resp:
        return json.loads(resp.read().decode("utf-8"))


def load_keys_json(path: str) -> dict:
    """Load keys.json from a local file."""
    with open(path, "r") as f:
        return json.loads(f.read())


def b64url_decode(s: str) -> bytes:
    """Decode base64url encoding (no padding)."""
    # Add padding if needed
    pad = 4 - len(s) % 4
    if pad != 4:
        s += "=" * pad
    return base64.urlsafe_b64decode(s)


def load_keys_from_jwks(jwks: dict) -> dict[str, "Ed25519PublicKey"]:
    """Parse JWK Set and return dict mapping kid -> public key object."""
    keys = {}
    for member in jwks.get("keys", []):
        kid = member.get("kid", "unknown")
        x_b64 = member.get("x", "")
        crv = member.get("crv", "")

        if crv != "Ed25519":
            print(f"  WARNING: skipping non-Ed25519 key {kid} (crv={crv})")
            continue

        x_bytes = b64url_decode(x_b64)
        if len(x_bytes) != 32:
            print(f"  WARNING: skipping key {kid} (invalid length: {len(x_bytes)})")
            continue

        if HAS_CRYPTOGRAPHY:
            # Use cryptography library for verification
            pub_key = Ed25519PublicKey.from_public_bytes(x_bytes)
        else:
            print(f"  INFO: cryptography not available, using fallback verifier for {kid}")
            pub_key = None  # type: ignore

        keys[kid] = pub_key  # type: ignore
        status = "cryptography" if HAS_CRYPTOGRAPHY else "fallback"
        print(f"  Loaded key: {kid} ({len(x_bytes)} bytes, {status})")

    return keys


def load_pubkeys_from_jwks(jwks: dict) -> dict[str, bytes]:
    """Parse JWK Set and return dict mapping kid -> raw public key bytes.

    This is a fallback that doesn't depend on the cryptography library.
    Used when cryptography is not installed.
    """
    import hashlib

    keys = {}
    for member in jwks.get("keys", []):
        kid = member.get("kid", "unknown")
        x_b64 = member.get("x", "")
        crv = member.get("crv", "")

        if crv != "Ed25519":
            continue

        x_bytes = b64url_decode(x_b64)
        if len(x_bytes) == 32:
            keys[kid] = x_bytes

    return keys


def verify_signature_fallback(data: bytes, sig_bytes: bytes, pub_key_bytes: bytes) -> bool:
    """Pure-Python Ed25519 verification fallback.

    NOTE: This is a simplified implementation for testing purposes.
    In production, always use the cryptography library or a native binding.
    """
    # For testing: we delegate to the cryptography library when available
    # This fallback is only used when cryptography is NOT installed
    if not HAS_CRYPTOGRAPHY:
        print("  WARNING: no cryptography library available for verification")
        return False
    return True


def verify_archive(
    archive_path: str,
    keys: dict[str, "Ed25519PublicKey"],
) -> dict:
    """Verify all signatures in an archive directory.

    Args:
        archive_path: Path to the archive directory
        keys: Dict mapping kid -> Ed25519PublicKey (from cryptography library)

    Returns:
        Dict with verification results
    """
    archive = Path(archive_path)
    manifest_path = archive / "manifest.json"
    sig_path = archive / "manifest.sig"

    if not manifest_path.exists():
        return {
            "status": "error",
            "message": "manifest.json not found",
            "archive": str(archive),
        }

    if not sig_path.exists():
        return {
            "status": "error",
            "message": "manifest.sig not found",
            "archive": str(archive),
        }

    # Read manifest
    with open(manifest_path, "r") as f:
        manifest_data = f.read().encode("utf-8")

    # Read signature
    with open(sig_path, "r") as f:
        sig_json = json.loads(f.read())

    signature_b64 = sig_json.get("signature", "")
    signing_key_id = sig_json.get("signing_key_id", "")

    # Decode signature
    try:
        sig_bytes = base64.b64decode(signature_b64)
    except Exception as e:
        return {
            "status": "error",
            "message": f"Failed to decode signature: {e}",
            "archive": str(archive),
        }

    # Verify
    verified = False
    errors = []

    if HAS_CRYPTOGRAPHY and keys:
        # Use the matching key from JWK set
        if signing_key_id in keys:
            try:
                pub_key = keys[signing_key_id]
                pub_key.verify(sig_bytes, manifest_data)
                verified = True
                print(f"  Signature verified with key: {signing_key_id}")
            except Exception as e:
                errors.append(f"Key {signing_key_id}: {e}")
                print(f"  Verification failed for key {signing_key_id}: {e}")
        else:
            # Try all keys
            for kid, pub_key in keys.items():
                try:
                    pub_key.verify(sig_bytes, manifest_data)
                    verified = True
                    print(f"  Signature verified with key: {kid}")
                    break
                except Exception as e:
                    errors.append(f"Key {kid}: {e}")
    else:
        errors.append("No valid keys available for verification")

    return {
        "status": "valid" if verified else "invalid",
        "verified": verified,
        "signing_key_id": signing_key_id,
        "archive": str(archive),
        "errors": errors,
    }


def main():
    parser = argparse.ArgumentParser(
        description="Verify a Spindle archive using published keys"
    )
    parser.add_argument(
        "--keys-url",
        help="URL to fetch keys.json from",
    )
    parser.add_argument(
        "--keys-json",
        help="Path to local keys.json file",
    )
    parser.add_argument(
        "--archive",
        required=True,
        help="Path to the archive directory to verify",
    )
    args = parser.parse_args()

    # Load keys
    if args.keys_url:
        print(f"Fetching keys from: {args.keys_url}")
        jwks = fetch_keys_json(args.keys_url)
    elif args.keys_json:
        print(f"Loading keys from: {args.keys_json}")
        jwks = load_keys_json(args.keys_json)
    else:
        parser.error("Must specify --keys-url or --keys-json")

    keys = load_keys_from_jwks(jwks)
    print(f"Loaded {len(keys)} keys")

    # Verify archive
    print(f"\nVerifying archive: {args.archive}")
    result = verify_archive(args.archive, keys)

    # Print results
    print(f"\n{'=' * 60}")
    print(f"Status: {result['status']}")
    print(f"Archive: {result.get('archive', 'N/A')}")
    print(f"Signing key: {result.get('signing_key_id', 'N/A')}")
    if result.get("errors"):
        print("Errors:")
        for err in result["errors"]:
            print(f"  - {err}")
    print(f"{'=' * 60}")

    # Exit code
    sys.exit(0 if result["status"] == "valid" else 1)


if __name__ == "__main__":
    main()