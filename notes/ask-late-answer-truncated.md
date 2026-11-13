# Late ask answer arrives truncated (2026-11-07)

Reported by an agent session, not investigated yet.

- A decision-sheet ask with about 20 answers, answered after the asking agent had moved on. Kova delivered the answer late by typing it into the agent's pane.
- The receiving session got only the end of the text: the start was lost.
- Suspect: a long late-answer message typed into the pane in one go (input buffer or paste chunking overrun while the agent is busy). To check in `src/ask.rs` (`late_answer_message`, delivery path): typed keys vs bracketed paste, and whether a long answer should be written to a file with only its path typed.
