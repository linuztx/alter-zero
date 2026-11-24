You are performing a CONTEXT CHECKPOINT COMPACTION. The conversation above is about to be replaced by your summary: another LLM will resume the task from it, with no access to anything you leave out. Write the handoff it can continue from without asking the user to repeat anything.

Cover, in this order, leaving out a section that has nothing in it:

1. The user's requests: every instruction the user gave, in their own words where the wording matters, with their preferences, constraints and corrections. Say which are done and which are still open.
2. Current state: what has been done so far. Files created, changed or deleted (with their paths), commands run and what they showed, decisions made and why.
3. Key facts: the specifics the next steps depend on. Exact names, paths, signatures, error messages, values, URLs, and the ids of background shells or agents still running. Quote short code or output verbatim where it will be needed again.
4. Problems and fixes: what failed, what was tried, what worked, and the dead ends not to repeat.
5. In progress: exactly what was being worked on when this checkpoint was taken, and the state it was left in.
6. Next steps: what remains, in order, starting with the very next action.

If the conversation already holds an earlier summary, carry forward everything in it that still matters: it is discarded with the rest.

The conversation is shown to you as a transcript shortened to fit. Where a long message or tool result has its middle missing, or old messages are missing, a line in square brackets says how much was left out of this transcript. Those cuts are this transcript's alone: the agent saw every message and result whole. An earlier summary in the conversation is likewise a full record of what came before it: take what it says as what happened.

So write the handoff as the agent's own notes about its work, never as a description of what you were shown. Do not mention the transcript, its cuts, an earlier summary, or what is or is not visible to you. Never say that something was truncated, incomplete, unread or unverified because of how it was shown, and never ask for something to be read or run again for that reason.

The next agent keeps the system prompt and the project instructions it was given, so leave them out. Write down what only this conversation holds.

Be specific and complete rather than brief, and do not pad. No preamble, no closing remarks, no tool calls: reply with the summary alone.
