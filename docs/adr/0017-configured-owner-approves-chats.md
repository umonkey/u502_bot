# ADR 0017: Configure the owner and approve chats from a direct message

- Status: accepted
- Date: 2026-11-22

## Context

Transcription spends credits and sends a chat's media to a third party, so it stays opt-in per chat. The opt-in was a list an operator maintained at startup, so every change meant finding the chat's identifier in the logs, editing the configuration, and restarting the bot. The bot already observes every chat it receives a message from, and the one fact it cannot observe is who is entitled to decide.

## Decision

Configure one owner once. The bot records every chat it receives a message from and enables transcription only where that owner approved it. The owner decides from a direct conversation with the bot, and nothing outside that record can enable a chat.

## Consequences

- An upgrade starts with every known chat unapproved, so each one needs a decision once.
- A decision takes effect immediately: no restart, no configuration change, and no operator reading logs.
- Activity in a chat never revives a blocked one, because recording a chat never overwrites a decision.
- A chat that is not approved still stores its messages but never spends credits.
- Approval has no effect on media that already arrived: an attachment that was skipped stays skipped.
- The interface works only in a direct conversation, and only once the owner has written to the bot, because a bot cannot start a conversation.
- The owner's decisions are kept indefinitely; retention applies to messages, never to the record of chats.

## Alternatives considered

- Keep the startup list: no new state, but every change still needs an operator, a restart, and a detour through the logs.
- Trust whoever writes in a chat first: no configuration at all, but any member could spend the bot's credits.
- Accept the commands in any chat: convenient in a group, but it hands the decision to every member.
- Buttons instead of typed commands: fewer typos, but it needs a second kind of update and a message that has to stay current.
