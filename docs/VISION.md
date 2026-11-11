# Persona Workshop vision

People should be able to shape how an agent communicates and approaches work
using guidance they can inspect and improve. Persona Workshop helps turn selected
evidence into portable communication and working-style profiles, then test those
profiles in situations that matter to the people using them.

The intended value is specific, useful guidance rather than generic personality
labels. A profile should help a person recognize a pattern, challenge an inference,
correct it, and understand why the guidance is there. Thin or contradictory
evidence should remain visible as uncertainty instead of producing a convincing
but unsupported portrait.

## Make the contribution reusable

Communication and working style are related but distinct. One concerns how
someone expresses themselves across contexts; the other concerns how they frame
problems, prioritize, make tradeoffs and carry work through. Keeping these
contributions separate lets a consumer use the guidance suited to its purpose.

A person can work with the evidence and versions here, then deliberately use a
profile in an independently owned agent or workspace. The consumer keeps its own
identity, permissions and execution responsibilities under the
[family agreement](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/family.v1-candidate.md).
The profile is guidance within that boundary, not a replacement for it.

## Support informed judgment

The service should make sources, attribution, revisions and comparisons usable
without requiring a person to reconstruct an analysis from raw logs. People can
contribute their expertise by judging examples and changing the guidance; agents
can assist with evidence processing and preparation within the agreed scope.

Evaluation should distinguish whether a profile loads correctly, whether a
response follows its guidance, and whether the intended person finds it faithful
and useful. Automated checks can establish particular boundaries. Human judgment
and fresh situations are needed to understand quality.

## Leave experiences open

Evidence curation, profile construction, comparison and portable delivery are
reusable service capabilities. The current workbench is development and
administration access. Other experiences can compose these operations in ways
that better fit their users without copying the service's private state or
assuming its current screen sequence is the future product.

The [contract map](contracts.md) locates the authoritative content, package and
service interfaces. Develop an idea against the relevant boundary and judge its
combined result with the consumer; a local export alone does not establish that
an installed agent used it or behaved well.
