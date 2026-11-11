# Verifying NEAR AI model responses

The bot runs in a TEE, but translations and transcriptions are produced by NEAR AI's
models in their own GPU TEEs. With `NEAR_AI__VERIFY_RESPONSES=true`, the bot checks that
side as well. The code is in `crates/near-ai-client/src/verify/`.

## What is checked

**Each chat (translation) response.** After the reply is sent, a background task:

1. Fetches the response's signature from `GET /v1/signature/{chat_id}`.
2. Requires `signature_kind = provider_tee`. This means the model instance's own key
   signed it, not NEAR AI's gateway. NEAR AI only produces these when the request sets
   `x-no-aliasing: true`, so the configured model must be a canonical name such as
   `deepseek-ai/DeepSeek-V4.1-Flash-1017`, not an alias.
3. Checks the ed25519 signature over `{model}:{sha256(request)}:{sha256(response)}`.
   The hashes are taken over the exact bytes the bot sent and received.
4. Checks that the signing key belongs to an attested instance of that model (below).
   An attestation is trusted for an hour. When a signer isn't known yet, the bot fetches
   up to four fresh reports, because NEAR AI load-balances across instances.

**Each model instance.** `GET /v1/attestation/report` is called with a fresh random
32-byte nonce. Then:

- **TDX quote:** verified against Intel's root of trust with `dcap-qvl`. This is the same
  library and version the agent contract uses. Collateral comes from Phala's PCCS, with
  Intel PCS as fallback. Revoked platforms and debug TDs are rejected. Intel's TCB status
  (e.g. `UpToDate`) and any advisories are recorded.
- **Report data:** must be the signing key followed by our nonce. This ties the key to
  this quote and the quote to this request.
- **Event log:** must replay to the quote's RTMR3. It must also record the hash of the
  reported app definition (`compose-hash`), and that hash must equal `compose_hash`. If
  MRCONFIGID is set, it must commit to the same hash.
- **GPUs:** the NVIDIA evidence must carry our nonce. NVIDIA's Remote Attestation Service
  must then return an overall pass, and report a nonce match and matching measurements
  for each GPU.

## When a check fails

Replies are never blocked or changed. Verification runs after the reply is sent, so it
adds no latency. An outage of NEAR AI's signature or attestation endpoints, Intel's
PCCS, or NVIDIA's NRAS doesn't take the bot down either.

Each failure is logged (`failed verification: …`) and counted per model, with the last
error kept. `!verify` reports these counts. The trade-off: a response that fails
verification has already been delivered. What the bot can say afterwards is how many
responses verified and how many didn't.

## What this does not prove

- **Whisper transcriptions are not signed.** NEAR AI returns no ID for a transcription
  and has no signature for one. For Whisper, the bot can only verify that an attested
  instance of the model exists. It can't verify that a particular transcript came from
  that instance.
- **The compose hash identifies the instance's app definition; it doesn't audit it.**
  NEAR AI's model instances run a compose manager that starts the model containers. We
  report the measured hash, but deciding whether that software is trustworthy is up to
  NEAR AI's published builds.
- **NRAS tokens are read, not signature-checked.** The verdict comes directly from
  NVIDIA's service over TLS, so it's trusted as NVIDIA's answer. The tokens aren't kept
  or passed on.
- **What the signature covers.** A `provider_tee` signature shows that the model
  instance signed this exact request and response. It doesn't show which weights
  produced the text, beyond what that instance's attestation covers.
