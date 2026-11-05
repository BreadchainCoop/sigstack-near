# Sigstack

TEE-hosted Signal bot for **voice transcription** and **group translation**, designed as an interoperable product suite.

## NEAR HACKATHON

Built by [Bread Cooperative](https://bread.coop) for **NEAR HACKATHON** (1–14 Nov 2026).

**Private AI you don't have to trust, verifiable from your Signal message to the GPU.**

- **Tracks:** Privacy & Private AI (`privacy`), AI Security & Defense (`security`)
- **Status:** in development

### What is new for the hackathon

1. **On-chain verification (Shade Agent).** The bot registers its TEE attestation with a NEAR
   contract that only accepts builds Bread has approved. Anyone can check on NEAR that the bot
   runs the published code, and every upgrade leaves a public record.
2. **Model verification.** The bot verifies NEAR AI Cloud's model attestation and response
   signatures, so `!verify` covers the whole path: your message, this bot's TEE, and the model's TEE.
3. **Optional: private payments through NEAR Intents.** Pay from any chain, with no card or name,
   and receive a `!link` code redeemed inside the TEE.

### Prior work and what is judged

Sigstack existed before the hackathon. It already ran in a Phala TEE and used NEAR AI for
Whisper and translation. Per the hackathon rules (Q-002 to Q-004), all of that is **baseline**.
**Only the NEAR work listed above, built during the hackathon window, is submitted for judging.**

- Baseline: [BreadchainCoop/sigstack-bot](https://github.com/BreadchainCoop/sigstack-bot) at
  [`b9b4a0e`](https://github.com/BreadchainCoop/sigstack-bot/tree/b9b4a0eda886d54aa808ab2fe597e6c9a3943cf9)
  (last commit 9 Oct 2026, before the hackathon was announced on 19 Oct). This repo carries its
  full history; that commit is tagged `pre-hackathon`.
- Hackathon work: everything after the `pre-hackathon` tag. Full diff:
  [`pre-hackathon...hackathon-submission`](https://github.com/BreadchainCoop/sigstack-near/compare/pre-hackathon...hackathon-submission)
  (until the submission tag exists: [`pre-hackathon...main`](https://github.com/BreadchainCoop/sigstack-near/compare/pre-hackathon...main))
- The hackathon build runs on a separate demo deployment. The production Sigstack bot is not
  changed by this repo.

### What the attestation pins

The demo runs [`docker/phala-demo.yaml`](docker/phala-demo.yaml). dstack measures that
file's text into the **compose hash**, which is what the agent contract approves. Env var
values are not measured, so the demo compose names every image by digest and writes
every setting into the file. Only real secrets (Signal phone, NEAR AI API key) come
from env.

To check a running bot yourself:

1. Get its attestation (`!verify` in Signal, or the verifier page). It includes
   `app_compose`, the exact compose the CVM booted.
2. sha256 of `app_compose` should equal the attested compose hash, and that hash
   should be one the agent contract approves.
3. Inside `app_compose`, read the image digests. Each is tagged `sha-<commit>` in
   `ghcr.io/breadchaincoop/sigstack-near-*`, built by the [`images`](.github/workflows/images.yml)
   workflow from that commit of this repo.

Every code change produces a new digest, so a new compose hash that must be approved
on-chain before the bot can register. Upgrades are public by construction.

The approved compose has no registration proxy, whose debug routes are unauthenticated.
The demo number is registered once with
[`docker/phala-demo-register.yaml`](docker/phala-demo-register.yaml), whose compose hash
is never approved, and the CVM is then upgraded in place to the demo compose.

### Try it

_To be written: the demo bot's Signal link, the public verifier page, and "do X, see Y" steps a
judge can follow._

### Libraries and tools

_Third-party libraries added for the hackathon work will be listed here (rule Q-005)._

### Other submissions

Bread Cooperative is also submitting
[saving-circles-near](https://github.com/BreadchainCoop/saving-circles-near), an independent
project (rule Q-015).

---

Not a general AI chat assistant. Conversation history, tool-calling, and x402 credits have been removed from this fork.

## Products

| Product | Where it runs |
|---------|---------------|
| Voice transcription | Same Phala CVM / same bot (NEAR AI Whisper; no local sidecar) |
| In-chat (group) translation | Same CVM (NEAR AI chat) |
| Language Threads | Same CVM |
| Bilingual Threads | Same CVM |

Add **one** bot to a Signal group.

**Organizer website:** [`site/`](site/) (SvelteKit → GitHub Pages) — https://breadchaincoop.github.io/sigstack-bot/ after Pages is enabled. See [site/README.md](site/README.md). Legacy [`web/`](web/) is not the product storefront.

Details: [docs/one-cvm-architecture.md](docs/one-cvm-architecture.md) · [docs/voice-transcription.md](docs/voice-transcription.md) · [docs/in-chat-translation.md](docs/in-chat-translation.md) · [docs/language-threads.md](docs/language-threads.md) · [docs/bilingual-threads.md](docs/bilingual-threads.md)

## Commands

| Product | Command | Effect |
|---------|---------|--------|
| Hub | `!help` / `!info` | Sigstack hub menus |
| Hub | `!privacy` | Privacy, TEE, and `!verify` (one reply) |
| Hub | `!translation-threads` | Language Threads menu |
| Hub | `!translation-in-chat` | In-chat translation menu |
| Hub | `!help-threads` | How Language Threads works |
| Hub | `!help-in-chat` | How in-chat translation works |
| Hub | `!help-transcription` | How voice transcription works |
| Voice | `!transcription` | Voice product menu |
| Voice | `!transcribe` / `!transcribe-on` / `!transcribe-off` | Manual or auto transcription (default off) |
| Language Threads | `!translate-me-thread <lang>` | Create/join sidecar (main only) |
| Bilingual Threads | `!translate-me-thread <main> <thread>` | Two langs, one sidecar, both-way translate (main only) |
| Language Threads | `!leave` | Leave this Language Thread (sidecar only) |
| Language Threads | `!commands` | Compact Language Thread command list (sidecar only) |
| Language Threads | `!enable-in-chat` | Tear down Language Threads (switch path to in-chat) |
| In-chat group-wide | `!translate-all-on <lang1> <lang2>` | Auto-translate all messages |
| In-chat group-wide | `!translate-all-off` | Disable group-wide auto |
| In-chat personal | `!translate-me-on <lang1> <lang2>` | Auto-translate this user’s messages only |
| In-chat personal | `!translate-me-off` | Clear this user’s personal auto |
| In-chat | `!enable-threads` | Clear all in-chat auto (switch path to Language Threads) |
| In-chat manual | `!translate <lang>` | Quote-reply translate one message |

Language Threads, Bilingual Threads, and in-chat auto are mutually exclusive. Details in the product docs above.

## Architecture

```
Signal group
   └── One Phala CVM (tdx.medium)
         └── signal-api + signal-bot (phone B)
               ├── audio bytes → NEAR AI Whisper Large V3 (GPU TEE)
               └── text → NEAR AI chat
```

- Signal E2E encryption terminates inside this TEE
- No local Whisper sidecar — see [CPU TEE Whisper does not scale](docs/solutions/architecture-patterns/2026-08-13-cpu-tee-whisper-does-not-scale.md)
- After STT, transcripts fan out in-process to in-chat / Language Threads (this number does not receive its own posts)

## Local

```bash
cp docker/.env.example docker/.env
# Set SIGNAL_PHONE; NEAR_AI_API_KEY (chat + Whisper STT)

docker compose -f docker/compose.yaml --env-file docker/.env up -d
```

More thorough local setup (Signal captcha registration, verify SMS/voice codes, and `docker compose logs -f` monitoring): [docs/local-dev/](docs/local-dev/).

## Phala

```bash
# In-place upgrade of the surviving CVM (phone B stays; do not create a replacement)
phala deploy --cvm-id 0e82fa77-8b15-4dbd-89c4-9045ab911353 \
  -c docker/phala.yaml -e docker/.phala.env --wait
```

**Do not replace this CVM or wipe its volumes** for a routine upgrade. Disk holds the **registered Signal phone** and **encrypted user prefs**. TEE RAM is cleared on reboot; Phala reattaches named volumes on in-place upgrade. Details: [docs/one-cvm-architecture.md — CVM storage](docs/one-cvm-architecture.md#cvm-storage-keep-intact).

After deploy / re-register, capture `username_token` from `phala logs` or `!bot-username` and set GitHub Actions variable `PUBLIC_SIGNAL_USERNAME_TOKEN` for the site Message button — [DEVELOPMENT.md — Signal username → site Message button](.agents/docs/DEVELOPMENT.md#signal-username-site-message-button).

## Project structure

```
crates/
  signal-bot/                   # Binary (hub + voice + translation)
  signal-bot-voice/             # Voice / !transcribe* product crate
  whisper-client/               # NEAR Whisper STT client
  near-ai-client/               # NEAR AI (chat + audio transcriptions)
  signal-client/                # Signal CLI REST client
  dstack-client/                # TEE attestation / key derive
  signal-registration-proxy/    # Ops helper for Signal registration
docker/
  compose.yaml                  # local one-number stack
  phala.yaml                    # prod one-CVM suite
  .env.example / .phala.env.example
docs/
  one-cvm-architecture.md       # one CVM / one phone; CVM storage
  in-chat-translation.md
  language-threads.md
  bilingual-threads.md
site/                           # educational storefront (GitHub Pages)
web/                            # legacy Private AI UI (not the storefront)
```

## Build & test

```bash
cargo build --release
cargo test
```

## Commit messages

This repo uses [commitlint](https://github.com/conventional-changelog/commitlint) with `@commitlint/config-conventional`. After clone, run `npm install` once so Husky can install hooks.

- **commit-msg** — rejects non-conventional messages at commit time  
- **pre-push** — rejects a push if any commit being pushed fails lint  

Format: `type(scope): subject` — e.g. `feat: add whisper timeout`, `fix(docker): pin signal-api digest`. Common types: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`.

## Configuration

| Variable | Description |
|----------|-------------|
| `SIGNAL__SERVICE_URL` | Signal CLI REST URL (default `http://signal-api:8080`) |
| `NEAR_AI__API_KEY` | Required (chat + remote Whisper) |
| `WHISPER__ENABLED` / `WHISPER__SERVICE_URL` | Required on the unified bot; URL is NEAR `/v1` |
| `TRANSLATE_ALL__ENABLED` | In-chat `!translate-all-on` / `!translate-me-on` |

See `.env.example` and the docker `*.env.example` files.

## Security

See [.agents/docs/DEVELOPMENT.md](.agents/docs/DEVELOPMENT.md) for the TEE trust model, why Signal CLI must run in the TEE, and attestation (`!verify`).

For coding agents, see [AGENTS.md](AGENTS.md) (includes Compound Engineering install: `/add-plugin compound-engineering`, then `/ce-setup`).

## License

Apache-2.0
