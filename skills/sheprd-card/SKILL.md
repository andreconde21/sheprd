---
name: sheprd-card
description: Keep the sheprd card ledger for the cards (tickets, issues, tasks) you work on, so the user sees status, open questions and decisions in the CARDS panel instead of reading your transcript. Use whenever you work on or orchestrate a card with an ID (SHE-12, CAL-40, #123), including the software-factory flow.
---

# sheprd card ledger

The user triages your work from the CARDS panel, not from your transcript. Write to it
explicitly; anything you don't write there they will not see. Commands target your own pane.
If `sheprd` is not on PATH, the same commands are `herdr card …` (or `"$HERDR_BIN_PATH" card …`).

```bash
sheprd card update SHE-12 --title "Stripe checkout" --status dev --summary "Checkout wired, webhooks next"
sheprd card ask SHE-12 "Which provider?" --option Stripe --option Multibanco --recommend 1
sheprd card decide SHE-12 "Use Checkout, not Elements"
sheprd card done SHE-12 --summary "Merged as #41; live on dev"
sheprd card answers          # answers the user sent you (marks them read)
sheprd card list             # your cards as the user sees them
```

## When
- **Start** of a card: `update` with its title and a status (`dev`, `review`, `blocked`…).
- **Status changes** and every notable step: `update --status/--summary`. The summary is the
  current state in one or two sentences (600 chars max), not a log.
- **A decision you took or the user gave**: `decide`, one line each (200 chars). These go to the
  card file when the card is done, so write them as the record: what, and why when not obvious.
- **A question for the user**: `ask` instead of (or as well as) asking in the chat. Keep working
  on what does not depend on it.
- **Finished** (PR open and handed over, or merged): `done --summary`. Done writes the summary and
  decisions back to the tracker when the user configured it; don't edit the card file yourself.
- Orchestrating subagents: you keep the ledger for the cards you hand out; subagents report to
  you, you write it.

## Questions
- One decision per question, answerable without your context: `"Charge monthly or yearly?"`,
  not `"What do you think?"`.
- Give 2-4 concrete `--option`s (80 chars each) and `--recommend N` (1-based) for the one you
  would pick; the user can still write a free answer.
- Answers arrive typed into your pane as one line, e.g.
  `SHE-12 Q1 (Which provider?) → Stripe · Q2 (Webhook path?) → "use the old endpoint"`.
  Run `sheprd card answers` to read them reliably, act on them, then `decide` what follows.

## Rules
- One line per field, no control characters. Card IDs: `A-Z a-z 0-9 # . _ -`, max 32 chars.
- Never put secrets in the ledger.
- If `sheprd` is missing or the server has no card ledger, carry on and say so once in the chat.
