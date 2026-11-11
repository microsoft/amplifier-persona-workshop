# Find the Persona Workshop boundary

The [vision](VISION.md) describes the intended value. Local commitments and exact
interfaces live in the documents below; this page only locates them.

| If the work changes… | Authoritative home |
| --- | --- |
| What communication or working-style guidance contains, which evidence is eligible, or how a revision is built | [Profile content](profile-content.md) |
| The portable package's required fields and file structure | [Package schema](../schemas/twin-profile.schema.json) |
| Export loading, preservation of existing workspace instructions, or consumer integration | [Portable exports](portable-exports.md) |
| Authentication, evidence/project operations, jobs, version application, evaluations or reference-client view control | [Service and view API](ui-api-contract.md) |
| Installed providers and operator settings | [Configuration](configuration.md) |
| How to exercise a boundary or assess profile quality | [Validation](validation.md) and [verification](verification.md) |

The service API owns operation shapes. The package schema owns portable data
structure. The profile-content contract owns the meaning of the instructions.
Examples illustrate those commitments; they do not establish another schema or
mandatory user journey.

Shared responsibilities, information ownership and self-contained installation
belong to the [Constellation family agreement](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/family.v1-candidate.md).
The [interaction agreement](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/experience.v1-candidate.md)
owns common control and outcome meanings; the
[SDK agreement](https://github.com/microsoft/amplifier-constellation/blob/main/contracts/protocol-sdk.v1-candidate.md)
owns the protocol integration policy. Refer to those homes rather than creating
local variants of their promises.
