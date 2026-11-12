// Package chat turns what a coding agent shows (a structured transcript or a captured terminal screen) into
// chat rows: safe HTML for markdown and ANSI output, tool calls and results, file paths as links, and the
// question an agent is blocked on with its choices.
//
// Screens: PickReader names the reader for an agent's screen (claude, devin, codex, agy, generic), Transform
// reads its turns and ScreenItems makes rows of them. Structured transcripts: ToItem converts one item. The
// package does no I/O and holds no mutable package state; every function is safe for concurrent use
// (MergeLocal writes only the firstSeen map its caller passes).
//
// Rows serialise as klark hub's API.md "Chat rows": {id, role, kind, html?, text?, time?, tool?, summary?,
// input?, ok?, toolUseId?, lines?}; Blocked as {question, options:[{label,send,enter}], choices?, selected?}.
package chat
