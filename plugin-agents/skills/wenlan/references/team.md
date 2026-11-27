# Team

A team is one Wenlan that stays on and hosts some Spaces for several people.
Everything here runs through the `wenlan` command on this computer. Resolve it
once:

```bash
W="$(command -v wenlan || echo "$HOME/.wenlan/bin/wenlan")"
```

Run the one command the request needs, then relay what it printed in the
user's language. Do not chain commands the user did not ask for.

| The user wants | Run |
|---|---|
| See the team here, or the teams this computer joined | `"$W" team status` |
| Start a team on this computer | `"$W" team start "<team name>"` (add `--as "<their name>"` when the command asks for it) |
| Help a person share a page with the team | Tell the person to use the app's **Share → With <team>** flow, or, in their own terminal, run `"$W" team create-space "<name>"` and then `"$W" team publish page "<ID>" --space-id "<SPACE ID>"`. The terminal publish shows a review and requires the person to type `yes`; do not run it on their behalf. |
| Invite someone | `"$W" team invite "<name>"` (add `--access "<Space>"` or `--access "<Space>:read"` only when the user named a Space for them) |
| A new link for someone who lost theirs | `"$W" team new-key "<name>"` |
| Join a team with an invite link | `"$W" team join "<link>"` |
| Leave a team | `"$W" team leave` (add the team's name when this computer is on more than one) |
| Remove someone | `"$W" team remove "<name>"` |
| Stop letting the team reach this computer | `"$W" team stop` |

## Rules

- Ask for what is missing (the team's name, the person's name, the link)
  instead of making one up.
- **An invite link is that person's key.** Show it once, exactly as printed,
  and tell the user to send it privately. Never put it in a memory, a page, a
  Brief or a session log, and never repeat it later in the conversation.
- **Remove, leave and stop change who can reach what.** Say in one line what
  will happen and run them only after the user said so in this turn. "Who is
  on the team?" is `status`, never `remove`.
- When a command fails, relay its message as it is: it already says what to
  do next. Do not retry with different arguments the user did not give.
- Do not publish or share on a person's behalf. The app flow and the terminal
  `publish` confirmation are actions for the person.
- The team gets the pages a person publishes, plus any cited memories that
  person ticks while reviewing. Other memories, sources and Spaces stay on
  each computer.
- A teammate who does not want to install Wenlan opens their invite link in a
  browser; it shows how to connect their AI tool.
