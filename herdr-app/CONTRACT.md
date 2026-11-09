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
      "evidence": [ { "kind": "test", "text": "unit: 37/0", "passed": 37, "failed": 0, "label": "", "at": "…" } ],
      "done": false, "done_at": null, "dismissed": false,
      "writeback": { "ok": true, "message": "wrote SHE-12.md" },
      "created_at": "…", "updated_at": "…"
    }
  },
  "inbox": { "t7": [ { "line": "SHE-12 Q1 (Which provider?) → Stripe", "at": "…", "read": false } ] }
}
```

- `questions[].state`: `open` → `staged` (an app chose an answer) →
  `sending` (claimed by one `cards send`) → `sent` (delivered with
  `hc send`), or `dismissed`. A failed send puts `sending` back to `staged`.
  An agent closes a question the user answered in the conversation with
  `cards answered <id> --question N [--answer TEXT]` (or `--all`): the state
  becomes `answered` (with `answered_at`, and `answer` when given). Open and
  staged questions can be closed this way; one being sent cannot.
  `replied` is a soft state a hook sets (`cards replied <id>:N ...`, only on
  `open` questions) when the user sent a chat message while the question was
  open and the agent did not close it: not live (not counted as unanswered),
  shown dim as "replied in chat". `cards answered` (or `--all`) still closes
  it, an app can `cards reopen <terminal> <id> --question N` (back to `open`,
  only from `replied`) or `dismiss` it, and staging an answer works as on an
  open one. `replied_at` records when.
  While `sending`, a question carries `claim` and `claimed_at`; a claim older
  than 120 s belongs to a send that died and may be staged and sent again
  (if that send had already typed the line, the agent gets it twice).
  Clients show `sending` as in progress and never write it themselves.
- Sending joins every staged answer of one terminal into one line, e.g.
  `SHE-12 Q1 (Which provider?) → Stripe · Q2 → "EUR only"`, delivered with
  `hc send <terminal>` and kept in `inbox` until the agent runs `cards answers`.
- `evidence` (newest 12): what the agent verified, added with `cards evidence <id>`.
  `kind: "image"` (`--image PATH`: png, jpeg, gif or webp, regular file, at most 2 MB, checked by its
  first bytes) is copied to `$SHEPRD_EVIDENCE_DIR` (default `~/.local/share/sheprd/evidence`, mode
  0700, files 0600) and stored as `file` (a bare name) + `mime` + `label`; the document never holds
  the bytes. `kind: "test"` (`--test "suite: P/F"`) keeps `text`, and `passed`/`failed` when the
  text ends in `P/F` (else `null`). `kind: "link"` (`--link URL`, http or https only) has `url` and
  `label` (default: the host). `--clear` removes them all. Images that fall off the list (or are
  cleared) are deleted from the folder. Images may hold at most 16 megapixels (read from the header; a gif checks every frame, all frames together at most 4 budgets).
  Clients open a link only when it is a plain http(s) URL (at most 2048 characters, no leading `-`, not
  localhost or a loopback, private, link-local or shared-range address) and open the parsed form:
  the ledger can be written by any process, so a stored URL is never trusted. Images are fetched at most
  a few per redraw. Apps fetch an image over the same `cards` channel as
  `cards json`: `cards evidence-image <id> <N> --terminal T` prints it as base64 (N counts from 1
  in `evidence`); it refuses anything but a stored regular image file. Show a test green when
  `failed` is 0, red when above 0, neutral when `null`; a missing image as its label.
- `dismissed` hides a card from views until its next update.
- `born` (cards and inbox entries) is the terminal's `created_ms`. When a new
  program starts in `t7` (a core restart reuses the id, or a new session), its
  first `cards` write adopts the open cards of the older `born` (their `born`
  becomes the new one, so a restarted session still sees its work) and drops
  the finished or dismissed ones, the open ones untouched for 7 days, and every
  answer in the `inbox` of the older `born` (read or not: it may answer someone
  else's question; each answer is also kept on its question).
- Cards written before `explicit` existed have no such list, so the first
  inferred write after an upgrade may overwrite a field an agent set earlier.
- `explicit` (card, optional): the fields an explicit `cards update` set. A hook's
  `cards update --inferred` fills only the other fields, never reopens a done
  card, and keeps `--inferred-question` entries (`inferred: true` on the question)
  separate from asked ones: each inferred write replaces the open inferred
  questions, `--clear-questions` drops them, an asked question is never touched.
- Pruning on every write: done cards 14 days after `done_at`; dismissed cards
  7 days and not-done cards 30 days after `updated_at`; per card all live
  questions plus the last 20 answered, the last 50 decisions; inbox entries
  after 14 days, read ones beyond the last 5. A write that hits herdr-core's
  1 MiB cap prunes hard (no done or dismissed cards, not-done cards older than
  7 days, last 5 answered questions, last 20 decisions, no read entries) and
  is retried once.

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
  "density": "detailed",
  "machine_tags": { "development-central": "dev" },
  "marks": {
    "unread": ["dev/t7"],
    "kept": ["omarchy/t3"],
    "dismissed": ["dev/t9@42"],
    "marked": ["dev/t4"]
  }
}
```

- Same shape as sheprd's `[[group]]` in `sidebar.toml` (imported from it on
  first run). `members` are `machine/<space name>` (`local/` for the hub),
  optionally followed by `#<space id>`. With the id, a member names that one
  space; without it, every space with that name. The sidebar writes the id
  form, because two spaces can share a name. Readers that don't know ids
  ignore the `#…` part;
  `match` are case-insensitive substrings of a space's name or folder.
  Members win over rules; the first matching project takes the space; the
  rest go to Other. `short` is the two-letter rail tag.
- `order` on a project (optional): agent addresses in the order the user
  dragged them; agents it doesn't list follow in the spaces' order.
- `filter`: `needs-you` | `working` | `marked` | `all`.
- `active_only` and `marks.kept`: sheprd's Active view. The app's sidebar no
  longer offers it (Marked covers it) and leaves them as they are.
- `muted` on a project, `other_muted` for Other: no attention sound or desktop
  notification for its agents (the sidebar plays them, per project, instead of
  Herdr's global sounds).
- `density`: `detailed` (two lines per agent: its work, then `space · machine`)
  | `compact` (one line, the space name). Missing means `detailed`.
- `machine_tags`: a short tag per machine name, shown instead of the host name.
  A machine without one gets its name's first part, cut to 3 letters when
  longer than 5 (`development-central` → `dev`); this machine gets none.
- `marks.highlighted`: addresses the user highlighted; only a look (a
  coloured bar and tint on the row), no filter or order. Missing means none.
- Marks follow sheprd's: `unread` clears on focus; `dismissed` is
  `<address>@<seq>` and lapses when the agent's `seq` moves on; `kept` stays in
  the active view; `marked` is the user's own flag on an agent (★), shown by the
  `marked` filter. The sidebar marks an agent it sees for the first time while
  the filter is `marked`, so a new agent stays in view. Older documents may
  lack `marked`: read it as empty.
- An agent "needs you" when blocked, done, or unread; a working agent with a
  staged-but-unsent answer also counts.
 Apps fetch an image over the same `cards` channel as
  `cards json`: `cards evidence-image <id> <N> --terminal T` prints it as base64 (N counts from 1
  in `evidence`); it refuses anything but a stored regular image file. Show a test green when
  `failed` is 0, red when above 0, neutral when `null`; a missing image as its label.
- `dismissed` hides a card from views until its next update.
- `born` (cards and inbox entries) is the terminal's `created_ms`. When a new
  program starts in `t7` (a core restart reuses the id, or a new session), its
  first `cards` write adopts the open cards of the older `born` (their `born`
  becomes the new one, so a restarted session still sees its work) and drops
  the finished or dismissed ones, the open ones untouched for 7 days, and every
  answer in the `inbox` of the older `born` (read or not: it may answer someone
  else's question; each answer is also kept on its question).
- Cards written before `explicit` existed have no such list, so the first
  inferred write after an upgrade may overwrite a field an agent set earlier.
- `explicit` (card, optional): the fields an explicit `cards update` set. A hook's
  `cards update --inferred` fills only the other fields, never reopens a done
  card, and keeps `--inferred-question` entries (`inferred: true` on the question)
  separate from asked ones: each inferred write replaces the open inferred
  questions, `--clear-questions` drops them, an asked question is never touched.
- Pruning on every write: done cards 14 days after `done_at`; dismissed cards
  7 days and not-done cards 30 days after `updated_at`; per card all live
  questions plus the last 20 answered, the last 50 decisions; inbox entries
  after 14 days, read ones beyond the last 5. A write that hits herdr-core's
  1 MiB cap prunes hard (no done or dismissed cards, not-done cards older than
  7 days, last 5 answered questions, last 20 decisions, no read entries) and
  is retried once.

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
  "density": "detailed",
  "machine_tags": { "development-central": "dev" },
  "marks": {
    "unread": ["dev/t7"],
    "kept": ["omarchy/t3"],
    "dismissed": ["dev/t9@42"],
    "marked": ["dev/t4"]
  }
}
```

- Same shape as sheprd's `[[group]]` in `sidebar.toml` (imported from it on
  first run). `members` are `machine/<space name>` (`local/` for the hub),
  optionally followed by `#<space id>`. With the id, a member names that one
  space; without it, every space with that name. The sidebar writes the id
  form, because two spaces can share a name. Readers that don't know ids
  ignore the `#…` part;
  `match` are case-insensitive substrings of a space's name or folder.
  Members win over rules; the first matching project takes the space; the
  rest go to Other. `short` is the two-letter rail tag.
- `order` on a project (optional): agent addresses in the order the user
  dragged them; agents it doesn't list follow in the spaces' order.
- `filter`: `needs-you` | `working` | `marked` | `all`.
- `active_only` and `marks.kept`: sheprd's Active view. The app's sidebar no
  longer offers it (Marked covers it) and leaves them as they are.
- `muted` on a project, `other_muted` for Other: no attention sound or desktop
  notification for its agents (the sidebar plays them, per project, instead of
  Herdr's global sounds).
- `density`: `detailed` (two lines per agent: its work, then `space · machine`)
  | `compact` (one line, the space name). Missing means `detailed`.
- `machine_tags`: a short tag per machine name, shown instead of the host name.
  A machine without one gets its name's first part, cut to 3 letters when
  longer than 5 (`development-central` → `dev`); this machine gets none.
- `marks.highlighted`: addresses the user highlighted; only a look (a
  coloured bar and tint on the row), no filter or order. Missing means none.
- Marks follow sheprd's: `unread` clears on focus; `dismissed` is
  `<address>@<seq>` and lapses when the agent's `seq` moves on; `kept` stays in
  the active view; `marked` is the user's own flag on an agent (★), shown by the
  `marked` filter. The sidebar marks an agent it sees for the first time while
  the filter is `marked`, so a new agent stays in view. Older documents may
  lack `marked`: read it as empty.
- An agent "needs you" when blocked, done, or unread; a working agent with a
  staged-but-unsent answer also counts.
