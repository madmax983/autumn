Plan:
1. Since the Persona is strictly an auditor ("Never do: Fix the docs yourself"), the goal is to produce the report in the expected issue format.
2. The format is:
   Title: `🗣️ Echo: [issue name]`
   Description:
   * 🤦 **The Confusion:** [The confusion point]
   * 🕵️ **The Reality:** [The reality of the situation]
   * 💡 **The Fix:** [The proposed fix]

Wait, the prompt says "Report findings via Issues/PRs". But I am an AI, so I just need to call the `submit` tool with this title and description, and some empty/dummy commit (since I shouldn't fix anything)?
Wait, the prompt says "Fix the docs yourself" is a "Never do", but usually in these tasks, if I'm not allowed to fix it, I just write an issue to a markdown file and commit that file, or I just use `submit` with the issue details.
Wait, let's look at `docs/stories/`. It has `S-001.md`, etc.
Maybe I should create an issue file in the root or in a new directory, or just use the `submit` tool directly without modifying any code?
Git requires some changes to commit.
Wait, if I create a file in `docs/stories/` or `issues/`?
Let me check `docs/stories/S-046-csrf-protection.md`. It's a standard story file.
If I am an auditor, I should probably just write my findings to an issue file in the repo (like `docs/issues/ECHO-001.md` or something) and commit it, OR I can just use `message_user`?
No, the instructions say "DX Audit PR/Issue convention (Echo persona): Title must be '🗣️ Echo: [issue name]'. Description must strictly use bullet points and specific emojis: ...". This perfectly matches the `submit` tool's arguments!
So I should call `submit` with `title` and `description` formatted correctly.
But to `submit`, I need git changes.
If I don't have git changes, `submit` might fail.
Wait, can I create a file called `ECHO_REPORT.md`? Let's check `AGENTS.md` if it exists.
