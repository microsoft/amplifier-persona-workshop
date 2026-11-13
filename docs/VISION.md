# Guidance that helps an agent work like you

Persona Workshop turns selected evidence into communication and working-style
guidance that people can inspect, correct and use in their agents. The value is
recognizable, useful guidance grounded in a particular person's behavior—not a
generic personality label or a convincing guess from thin evidence.

A person can select examples, review what they suggest, try the resulting profile
in a fresh situation and revise it. Communication captures expression and its
context. Working style captures how the person frames problems, weighs tradeoffs
and carries work through. Consumers can use either or both.

## Make judgment and reuse practical

Sources, uncertainty and revisions should be understandable without reconstructing
the analysis from raw logs. People bring expertise by challenging interpretations
and judging examples; agents assist with evidence processing and preparation.
A saved profile can then be deliberately applied to an existing agent or exported
to another workspace. It supplies guidance, not identity or permission.

Persona Workshop combines an
[Experience](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/experience.md)
for curating and judging guidance with reusable
[Tool capabilities](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/tool.md)
for profile construction, evaluation and export. Other experiences can use the same
saved versions through the service API. A profile is a portable artifact, not a new
Agent Instance or a copy of the person. Consumers choose how guidance fits their
internal design; the current workbench does not prescribe another product's UX.

Judge success separately at three levels: the profile loads correctly, a response
uses its guidance, and the intended person finds the result faithful and useful.
A package or review label alone cannot establish all three.

The [local agreement](contracts.md) connects profile meaning to consumer use.
Shared behavior stays in the [Constellation agreements](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/README.md).
