# Persona Workshop agreements

<a id="find-the-persona-workshop-boundary"></a>

The [vision](VISION.md) explains the value. Two local boundaries carry the promises
specific to this component; choose the one your change affects. The workbench
provides an [Experience](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/experience.md);
service operations provide [Tool capabilities](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/tool.md).
Profiles are versioned artifacts used by agents, not Agent Instances themselves.

| Agreement | What a consumer can rely on |
| --- | --- |
| [Profile content](profile-content.md) | What communication and working-style instructions mean, eligible evidence, review preservation and revision validity. |
| [Portable exports](portable-exports.md) | What leaves the service, how a saved version loads, how existing workspace instructions are preserved, and privacy/compatibility limits. |

**Example:** A person corrects a working-style instruction after a fresh scenario.
Profile content determines how rebuilding preserves that decision. Portable
exports determines what a consumer receives when that saved version is deliberately
applied. Neither operation establishes that the consumer's behavior is faithful;
that requires a separate evaluation.

## Exact interfaces and checks

- [Package schema](../schemas/twin-profile.schema.json): required data and files.
- [Service API](ui-api-contract.md): authentication, projects, evidence, builds,
  application, evaluations and current reference-client view operations.
- [Configuration](configuration.md): providers and operator settings.
- [Validation](validation.md) and [verification](verification.md): executable
  checks and the limits of their quality claims.

Shared [composition](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/composition.md),
[interaction](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/interaction.md)
and [engine compatibility](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/engine-compatibility.md)
promises stay with Constellation. They are not person-specific profile instructions.
The API owns operation shapes, the package schema owns structure, and the two
local agreements own content and consumption meaning. This map adds no second
schema, required screen sequence or universal claim of persona fidelity.
