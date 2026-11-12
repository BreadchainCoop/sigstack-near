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

dstack hashes `app_compose`, a JSON document that wraps the compose file together with
the CVM settings and Phala's pre-launch script. The demo's is committed byte-for-byte as
[`docker/phala-demo.app-compose.json`](docker/phala-demo.app-compose.json), copied from
the CVM's attestation and refreshed after every demo upgrade. Every build ever approved is also
archived under [`verifier/builds/`](verifier/builds), indexed by
[`verifier/builds.json`](verifier/builds.json). To check what the demo bot runs:

1. `sha256sum docker/phala-demo.app-compose.json` gives the compose hash. It should equal
   the `app_compose_hash_payload` the agent contract approved and verified when the bot
   registered, and the compose hash `!verify` reports (see [Try it](#try-it)).
2. `jq -j .docker_compose_file docker/phala-demo.app-compose.json | diff - docker/phala-demo.yaml`
   prints nothing: the attested compose is this repo's file.
3. In that file, read the image digests. Each is tagged `sha-<commit>` in
   `ghcr.io/breadchaincoop/sigstack-near-*`, built by the [`images`](.github/workflows/images.yml)
   workflow from that commit of this repo. Images built since that workflow started signing
   provenance also carry a GitHub attestation of the commit and run. Check its signature with
   `gh attestation verify oci://<image>@<digest> --repo BreadchainCoop/sigstack-near`.

For an older build, use its archived file in step 1 and its `compose_commit` from
`builds.json` in step 2 (`git show <compose_commit>:docker/phala-demo.yaml`). The rest of
`app_compose` is the CVM settings and Phala's pre-launch script. They can't be regenerated
from this repo, so they're checked as published. On every change to the archive, CI runs
[`verifier/check_builds.py --git --registry`](verifier/check_builds.py). It checks all
three steps for every build, plus the provenance signature for builds recorded with
`"provenance": "github"`.

Every code change produces a new digest, so a new compose hash that must be approved
on-chain before the bot can register. Upgrades are public by construction.

The approved compose has no registration proxy, whose debug routes are unauthenticated.
The demo number is registered once with
[`docker/phala-demo-register.yaml`](docker/phala-demo-register.yaml), whose compose hash
is never approved, and the CVM is then upgraded in place to the demo compose.

### Try it

Four ways to check this work. They're ordered from least to most setup.

#### 1. Run the tests (no TEE, no NEAR account, no secrets)

Needs [rustup](https://rustup.rs) and Docker.

```bash
./scripts/test_contract.sh   # agent contract: unit tests, wasm build, near-sandbox integration tests
cargo test --workspace       # bot crates, including near-agent (attestation + registration)
```

CI runs the same commands: the [`contract`](.github/workflows/contract.yml) workflow runs the
script, and the [`test`](.github/workflows/test.yml) workflow runs the bot tests with a ≥90%
line-coverage gate. The contract's wasm is built in the same pinned `cargo-near` image CI uses,
so it builds the same on Linux and macOS.

What the contract tests cover (details in [`contracts/agent/README.md`](contracts/agent/README.md#testing)):

- **Approvals:** approving and revoking measurements (which include the compose hash) and PPIDs.
  Only the owner can do either.
- **Registration:** registering with approved measurements, and rejection when the measurements
  or PPID aren't approved.
- **Expiry and pruning:** an expired or revoked agent can be pruned by anyone and must re-register.
- **TEE mode:** named accounts are rejected, because the quote binds the agent's implicit
  account. The mock attestation accepted in local mode fails real quote verification.
- **A real quote:** the demo bot's actual `register_agent` arguments (its Phala TDX quote, PCCS
  collateral and `tcb_info`) replay at the block time they executed. They register the agent
  when its build and PPID are approved. They're rejected from any other account, for an
  unapproved build or PPID, and once the collateral is out of date.
- **Upgrades:** `update_contract` keeps the owner, settings and approvals.

The `near-agent` tests cover:

- building the attestation from dstack's `tcb_info` and quote;
- fetching collateral from PCCS;
- signing and sending NEAR transactions;
- the register-and-refresh loop.

They run against mock dstack, PCCS and RPC servers.

#### 2. Check the live contract on NEAR testnet (read-only)

The quickest way is the **[verifier page](https://breadchaincoop.github.io/sigstack-near/)**. It reads
the contract live and shows the registered bot and every approved build. For each build it hashes
the published `app_compose` in your browser. It links the commit whose compose file it is, and
lists the image digests the build pins, each with what GitHub's signed provenance says built it. It also
checks the contract's on-chain code against the CI-built wasm, and says what is and isn't verified.
It's one static file, [`verifier/index.html`](verifier/index.html), and needs no tooling. Opened
locally, it fetches the build files from `main`.

To check the same things from a terminal: the agent contract is [`agent.sigstack.testnet`](https://testnet.nearblocks.io/address/agent.sigstack.testnet),
owned by `sigstack.testnet`. With [near-cli-rs](https://github.com/near/near-cli-rs):

```bash
# Registered bots, with the measurements they attested and when they expire
near contract call-function as-read-only agent.sigstack.testnet get_agents json-args '{}' network-config testnet now
# Builds the owner has approved (the compose hash is app_compose_hash_payload)
near contract call-function as-read-only agent.sigstack.testnet get_approved_measurements json-args '{}' network-config testnet now
```

The deployed code is traceable to this repo:

- **Source:** the wasm the [`contract`](.github/workflows/contract.yml) workflow built from
  commit `66f4b00` ([run 42946350559](https://github.com/BreadchainCoop/sigstack-near/actions/runs/42946350559)).
- **wasm sha256:** `9890b61e434341c2c2b995e71690c054edd0ed0b960da421e267771366017667`, as logged
  by that run.
- **On-chain code hash:** `BGYxJkzMBrZZZ7Ju8D9UDV7rNpXfuw3ZG7de7xrAUPUA`. This is the base58 form
  of the same sha256. Check it with
  `near account view-account-summary agent.sigstack.testnet network-config testnet now`.

The demo bot is registered. At the time of writing, its agent account is
[`f901a4ed…6409`](https://testnet.nearblocks.io/address/f901a4ed426e431693ab2337c2b1db87923d1d79883e8e92ae6068a67e546409),
registered in
[`4tcoMTYb…C75m`](https://testnet.nearblocks.io/txns/4tcoMTYbd3Vtf1st9VTWQd8G4ijNL6ADN3N4NMwBC75m),
relayed by the sponsor as a NEP-366 meta-transaction.
The bot makes a new key on every boot, so after a restart the current one is the newest entry in
`get_agents`. Registration lasts 7 days, and the bot re-registers before it expires.

To tie the registered bot to this repo:

1. In `get_agents`, its `measurements.app_compose_hash_payload` is the compose hash the contract
   checked against the TDX quote. For the demo it is
   `5f0f9eb93ca24ec56a5fe9e6cf4adc10ead8ea4da5d02be893d990c91ee1cd6b`.
2. That hash is the sha256 of
   [`docker/phala-demo.app-compose.json`](docker/phala-demo.app-compose.json), which wraps
   [`docker/phala-demo.yaml`](docker/phala-demo.yaml) unchanged. See
   [What the attestation pins](#what-the-attestation-pins) for the commands.

#### 3. Talk to the demo bot in Signal

Open [signal.me/#eu/SbV9bgtt…](https://signal.me/#eu/SbV9bgttO6r-3Zpak8AUB3WdIeIjgLsjAShkM6lcgQPihTCX4xJIpJnTmXFW4IwW)
on a device with Signal, or search for the username `sigstackdemo.98`. The demo doesn't need a
subscription; every command is open.

To check you're talking to the attested bot, send `!verify <random text>`, e.g. `!verify k3v9-pear`.
The bot answers with two messages. The first has:

- **Your challenge:** `Sigstack: k3v9-pear`. The bot prefixes your text, then puts it in the
  quote's report data.
- **Report data:** the challenge in hex. Check it with
  `printf '%s' 'Sigstack: k3v9-pear' | xxd -p`. A challenge over 64 bytes is SHA-256 hashed first.
- **Compose hash:** should be `5f0f9eb9…cd6b`, the same as above.
- **How to verify:** the same three checks as here, with links to the verifier page and this repo.

The second message is the TDX quote alone. Long-press it, copy it, and paste it into
[proof.t16z.com](https://proof.t16z.com). It checks the Intel signature. Its report data
should be your hex, zero-padded to 64 bytes. A fresh challenge in a genuine quote proves the
reply came from inside the TEE just now.

Then try the product:

- **Voice, in a DM or a group:** `!transcribe-on`, then send a voice note. It's transcribed by
  NEAR AI Whisper.
- **Translation, in a group with the bot:** `!translate-me-on en es`, then write in either
  language. Your messages are translated into the other one.
- `!help` lists everything. See [Commands](#commands).

#### 4. Run your own bot (optional)

You need your own Signal number and a NEAR AI API key. See [Local](#local). Outside a TEE the
bot runs but doesn't register with the agent contract.

### Libraries and tools

Third-party code added for the hackathon work (rule Q-005):

- **Agent contract** ([`contracts/agent`](contracts/agent)):
  - Adapted from `shade-contract-template` in
    [NearDeFi/shade-agent-framework](https://github.com/NearDeFi/shade-agent-framework) at
    `35758f3` (Apache-2.0).
  - [`near-sdk`](https://crates.io/crates/near-sdk) 5.26.1.
  - `shade-attestation` from the same framework commit (MIT). It brings in
    [`dcap-qvl`](https://github.com/Phala-Network/dcap-qvl) 0.5.2 for TDX quote verification and
    `dstack-sdk-types`.
  - Tests: [`near-api`](https://crates.io/crates/near-api) 0.8.6,
    [`near-sandbox`](https://crates.io/crates/near-sandbox) 0.3.11 and `tokio`.
- **Bot** ([`crates/near-agent`](crates/near-agent)):
  - No new third-party crates. It uses `ring`, `reqwest`, `serde_json`, `base64`, `sha2` and
    `hex`, which were already in the lockfile.
  - It implements dcap-qvl's collateral-fetch protocol, NEAR transaction encoding and NEAR
    JSON-RPC directly.
- **Tools:**
  - [`cargo-near`](https://github.com/near/cargo-near) in the pinned
    `sourcescan/cargo-near:0.19.0-rust-1.86.0` image.
  - [near-cli-rs](https://github.com/near/near-cli-rs).
  - GitHub Actions.
- **Services:**
  - [Phala Cloud](https://phala.com) (dstack TDX CVMs).
  - Phala's PCCS and Intel's PCS (quote collateral).
  - NEAR testnet RPC ([FastNEAR](https://fastnear.com)).
  - [NEAR AI Cloud](https://cloud.near.ai).

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
cargo test --workspace          # bot crates
./scripts/test_contract.sh      # agent contract (needs Docker); see "Try it" above
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
