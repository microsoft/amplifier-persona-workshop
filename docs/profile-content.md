# Profile content

The two Markdown files are reusable instructions about a particular entity. They are not an extraction report, a set of generic agent policies, or a biography. Keep the identity bindings, analysis, provenance, confidence, review state, and evaluation results outside these files.

## Communication

Extract the features that would make an agent's language sound different: word choices, openers and their purpose, shorthand, punctuation, cadence, message shape, degree of directness, warmth, disagreement, and changes by medium or audience. Ground short examples in the supplied material. Do not turn a recognizable phrase into a mandatory opener for every message.

For example, “Be clear and collaborative” is too generic. “In an informal design chat, begin with a brief reaction, state the concern, then explain why it matters; leave a related detail in a parenthetical rather than opening a second argument” describes a reproducible pattern. Such guidance belongs in a profile only when the particular subject's material supports it.

The voice-analysis approach in [amplifier-bundle-my-voice](https://github.com/microsoft/amplifier-bundle-my-voice) informs this distinction: preserve characteristic expression, capture context, and condense repetition without replacing the person's voice with polished generic prose. Its reporting and setup instructions are not part of the exported runtime files.

## Thinking and working

Extract how the subject frames problems, makes decisions, allocates attention, handles competing priorities, executes or delegates work, and changes direction. Capture conditional preferences and tensions. Describe transferable processes, not current project facts or temporary assignments. Do not repeat the communication profile or substitute general management advice.

## Sources and generation

- Authored samples contribute observable usage. Only the selected person's contributions are eligible; quotes, assistant output, and held-out material stay out.
- Existing profiles contribute explicitly supplied prior guidance. Their document-writing style is not evidence of the subject's voice. They are not counted as independent authored corroboration.
- AI conversations default to working-style evidence. The owner may deliberately reuse a source in both collections.
- Extracted `statement` fields are direct, context-aware instructions. Any necessary condition belongs in the instruction itself, not only in the separate `scope` field.
- `section` organizes instructions under short headings. Evidence references, confidence and scope remain in `claims.json` and optional workbench details.
- Missing patterns remain absent. An empty collection exports its title without invented instructions or generic filler.

A build stores its observations, compiled profile version, and completed job in one transaction. A changed project makes the build stale instead of overwriting changes. Rebuilding retains owner rules, accepted/rejected decisions, and edited pending rules. Versions are immutable; source/privacy changes can invalidate them for further use.

## Consumer responsibilities

Consumers load `communication.md` and/or `working-style.md` as guidance, with their own identity, permissions, execution and disclosure policies. Those general policies do not belong in person-specific profiles. The workbench's scenario runner supplies its own simulation boundary and never treats simulated replies as permission to act. Profile quality is judged from actual examples and user feedback, not inferred from review labels.
