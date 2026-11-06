# Voice typing: how Claap does it, what Kova copies

Read on 06/11/2026 from the Claap monorepo, branch `dev` (CLA-10105, back
CLA-10118, PRs #13752, #13753, #13770 merged, not in prod), and
`Claap/Product/docs/dictation-desktop/` (spec, copy).

## Claap

- **Endpoint**: `POST /voice-typing/dictate` on the GraphQL HTTP API
  (`https://api-dev.claap.io/voice-typing/dictate` on dev), a Lambda behind
  API Gateway, 29 s timeout. `GET /voice-typing/gating?workspaceId=` says
  whether the workspace may dictate (rollout flag `voiceTyping`, the
  workspace toggle, a provider key).
- **Protocol**: REST, one buffered request per dictation, no streaming.
  Multipart: `config` (JSON: `dictationId` UUID, `workspaceId`,
  `languageCodes` up to 3) and `audio` (raw 16 kHz mono int16 PCM, 120 s
  at most). Answer: `{dictationId, text, rawText, cleanupFailed,
  audioDurationMs}`; errors `{error: {type, message}}` with 400, 401, 403,
  429 (daily ceiling), 503.
- **Auth**: Cognito tokens of the user's session, the access token raw in
  `Authorization` and the id token in `x-claap-id-token`, from the same user pool and app client as the web
  app (one per stack, dev and prod). The desktop
  gets them from the web app through its PKCE flow (`/api/authorize`, the
  launch page, then `claap://login?code=`), stores them encrypted with
  Electron safeStorage in LevelDB, refreshes them with `REFRESH_TOKEN_AUTH`.
  The client has no hosted UI and allows custom auth only (SRP password or
  Google token), so a third app has no clean way in without a back change.
- **Ceiling**: 1 h of audio and 2,000 requests per account per UTC day;
  refunded when the provider did not bill.
- **Provider**: AssemblyAI's dictation endpoint, EU data zone,
  `https://dictation.eu.assemblyai.com/v1/transcribe/live`, key held by
  the back only. Config sent: `channels` 1,
  `sample_rate` 16000, `language_codes`, `keyterms_prompt` (vocabulary),
  `llm_instruction` (cleanup rules), `rewrite_mode` `fast`. 3 attempts on
  429, 502, 503, 504 or no connection; a 400 with a vocabulary is retried
  once without it.
- **Cleanup**: server side, by the provider's LLM in the same call, with
  Claap's instruction v6 (`voiceTypingRules.ts`, 3,213 characters: drop
  hesitations and stutters, keep the last version of a self-correction,
  light punctuation, no dashes, never answer, spoken commands). `text` is
  the verbatim transcript, `llm_response` the clean one; without it the
  verbatim text is pasted.
- **Vocabulary**: per dictation, the workspace glossary (recent first),
  then calendar people and the user, 100 terms and 7,500 characters at
  most; after the answer, the glossary's known misspellings are replaced.
- **Desktop client**: not on `dev` yet (only settings). Per the spec:
  hold `fn` over 300 ms (push-to-talk) or hands-free, 9 green bars in a
  pill, sounds off by default, Escape cancels, history on the Mac.

## Kova

Same provider, endpoint, config and limits, called directly
with the user's own AssemblyAI key (Keychain `assemblyai-api-key`); no Claap
account, no ceiling, no gating. Vocabulary from `[chat] voice_vocabulary`,
no glossary misspelling pass. Claap's cleanup rules are read from
`~/.config/kova/voice-instruction.txt`, kept out of this public repo; without
the file a short generic set applies. Languages fixed to French and English.
Details of the experience: `chat-mode.md`, Dictation.
