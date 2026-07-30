---
description: Review code changes for bugs, security issues, and style problems
---
# Code Review Skill

When asked to review code, follow this workflow:

1. Run `git diff` to see all changes
2. Check for security issues (path traversal, command injection, secrets)
3. Look for logic errors and edge cases
4. Verify tests exist for new behavior
5. Provide findings as: `file:line: severity: problem. fix.`

Be concise and actionable. Do not praise — only report issues.
