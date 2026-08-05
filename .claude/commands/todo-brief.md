---
description: Scan Slack and Gmail for this week's action items and write the briefing the notch HUD shows
allowed-tools: Bash(notch todos*), Write, Read, mcp__claude_ai_Slack__slack_search_public_and_private, mcp__claude_ai_Slack__slack_search_public, mcp__claude_ai_Slack__slack_search_channels, mcp__claude_ai_Slack__slack_read_channel, mcp__claude_ai_Slack__slack_read_thread, mcp__claude_ai_Slack__slack_search_users, mcp__claude_ai_Slack__slack_read_user_profile, mcp__claude_ai_Gmail__search_threads, mcp__claude_ai_Gmail__get_thread, mcp__claude_ai_Gmail__get_message
---

You are producing a to-do briefing from Slack and Gmail, and writing it where the
notch HUD reads it.

The HUD itself has no Slack or Gmail access and never will — it only renders the
file you write. You are the part with the connectors, so the judgement about what
counts as an action item is yours.

## 0. Check you can actually do this

**Run this only in a session that has both connectors.** A headless `claude -p` has
no Slack or Gmail tools at all, so this cannot be driven from cron.

Before scanning, confirm you can see `slack_*` and Gmail tools:

- **Neither available** — stop. Say so and write nothing. A stale briefing left in
  place is better than an invented one.
- **Only one available** — carry on with that source alone, and say which one is
  missing. Set `source` to just the source you used, so the HUD isn't claiming
  coverage you didn't have.

Never infer items from memory to fill a gap.

> The tool names above are from one particular setup. If this session's MCP servers
> are named differently, the `allowed-tools` list needs adjusting to match.

## 1. Gather — the week, both sources

Cover **the last 7 days**, and anything explicitly due this week.

**Slack.** Scan the channels available to you for messages relevant to the user.
Prioritise:

- direct mentions of them, their handle, or their name
- threads they took part in that are waiting on their reply
- asks: "can you", "please", "could you", "needs", "by Friday", "deadline",
  "blocked on"

Skip channels with nothing relevant. Read a thread when a hit needs its context to
be actionable.

**Gmail.** Search the last 7 days for mail that wants a reply or an action.
Prioritise:

- threads where the user is a direct recipient and has not replied
- explicit requests, approvals, or deadlines
- anything already flagged, starred, or in a follow-up label

Skip newsletters, notifications, receipts, calendar invitations, and automated
alerts — those are not action items. Open a thread only when the subject line alone
doesn't say what's being asked.

Prefer the last few days for today's items and the whole week for the rest.

## 2. Classify into exactly four buckets

- **today** — needs attention today
- **week** — due or committed to later this week
- **in_progress** — actively being worked on, or waiting on someone else
- **done** — explicitly completed, resolved, or closed out

Each item is one concrete, actionable sentence. Not "tariff thread" but "Reply to
Kenji about the tariff numbers".

For each, record where it came from:

- `channel` — the Slack channel (`#eon`) or, for mail, the mailbox or label
  (`inbox`, `follow-up`)
- `who` — the person who asked, when there is one
- `url` — the Slack permalink or the mail thread link, when you have it

**Never invent an item to fill a section.** Empty is a real and useful answer, and
the HUD renders it as one.

De-duplicate across sources: the same request raised in Slack and chased by email is
one item, not two. Keep whichever source carries the clearer ask.

## 3. Write it

Write JSON to the path `notch todos path` prints, creating parent directories if
needed, in the shape `notch todos schema` prints:

```json
{
  "generated_at": "<now, RFC3339 UTC>",
  "source": "slack+gmail",
  "today":       [{ "text": "...", "channel": "#...", "who": "..." }],
  "week":        [],
  "in_progress": [],
  "done":        []
}
```

`generated_at` must be the real current time. The HUD shows the briefing's age and
marks it stale after 36 hours, so a wrong timestamp actively misleads — it will
present old items as today's.

Set `source` to what you actually read: `slack+gmail`, or `slack`, or `gmail`.

## 4. Report

Run `notch todos` and show its output, so the four sections are visible in the
transcript as well as in the notch.

Then say plainly:

- which Slack channels you scanned, and roughly how much mail
- which source each item came from
- anything you deliberately skipped, and why

If the panel is open, the To-do tab will pick the new briefing up within a few
seconds. `notch module todos on` first, if that module is off.
