# sheprd on herdr-core: shared documents (contract v1)

The sheprd mods for the Herdr app, the `cards` CLI and Conductore share two
herdr-core documents on each machine. Read them with `doc.get` and write them
with `doc.set` (version-checked) over `hc connect` (JSON-RPC lines on
stdin/stdout), or through the `cards` CLI. A write that loses the race gets
`error.data.kind = "conflict"`: read again and retry.

Addresses: a terminal is `t7` on its own machine. Clients that see several
machines prefix the machine name (`dev/t7`) and strip it when writing to that
machine's document.

## `sheprd-cards`: the card ledger

Written by agents through `cards` (update / ask / decide / done / answers) and
by apps through `cards stage / dismiss / send`.

```json
{
  "v": 1,
  "cards": {
    "t7/SHE-12": {
      "id": "SHE-12", "terminal": "t7",
      "title": "Stripe checkout", "status": "dev", "summary": "…", "link": null,
      "questions": [
        { "n": 1, "text": "Which provider?", "options": ["Stripe", "Multibanco"],
          "recommend": 1, "state": "open", "answer": null, "asked_at": "…" }
      ],
      "decisions": [ { "text": "Use Checkout, not Elements", "at": "…" } ],
      "done": false, "done_at": null, "dismissed": false,
      "writeback": { "ok": true, "message": "wrote SHE-12.md" },
      "created_at": "…", "updated_at": "…"
    }
  },
  "inbox": { "t7": [ { "line": "SHE-12 Q1 (Which provider?) → Stripe", "at": "…", "read": false } ] }
}
```

- `questions[].state`: `open` → `staged` (an app chose an answer) → `sent`
  (delivered with `hc send`), or `dismissed`.
- Sending joins every staged answer of one terminal into one line, e.g.
  `SHE-12 Q1 (Which provider?) → Stripe · Q2 → "EUR only"`, delivered with
  `hc send <terminal>` and kept in `inbox` until the agent runs `cards answers`.
- `dismissed` hides a card from views until its next update.
- Done cards are pruned 14 days after `done_at`.

## `sheprd-view`: the sidebar layout

Written by the `sheprd-sidebar` mod; read by Conductore. Kept on the machine
the app runs on (the hub), like sheprd's `sidebar.toml`.

```json
{
  "v": 1,
  "projects": [
    { "name": "Conductore", "short": "CO", "pinned": false, "collapsed": false,
      "members": ["dev/Conductore-Mobile"], "match": ["conductore"], "note": null }
  ],
  "hidden": ["dev/w12"],
  "filter": "needs-you",
  "active_only": true,
  "other_collapsed": true,
  "marks": {
    "unread": ["dev/t7"],
    "kept": ["omarchy/t3"],
    "dismissed": ["dev/t9@42"]
  }
}
```

- Same shape as sheprd's `[[group]]` in `sidebar.toml` (imported from it on
  first run). `members` are `machine/<space name>` (`local/` for the hub);
  `match` are case-insensitive substrings of a space's name or folder.
  Members win over rules; the first matching project takes the space; the
  rest go to Other. `short` is the two-letter rail tag.
- `filter`: `needs-you` | `working` | `all`.
- Marks follow sheprd's: `unread` clears on focus; `dismissed` is
  `<address>@<seq>` and lapses when the agent's `seq` moves on; `kept` stays in
  the active view.
- An agent "needs you" when blocked, done, or unread; a working agent with a
  staged-but-unsent answer also counts.
