## Schema files

The schema directory serves 2 purposes:
 1. It create the schema to validate the various configuration files
 2. It specifies the default values which, in turn, can be used to create a default file

Therefore it is important to properly document all the nodes, as the documentation
will allow generating the comments of the generated files.

All schema files must end with **.schema.yaml**.

## Persistence strategy

Configuration and state are persisted in the user's config directory:

### Global configuration
- **File:** `global.setting.yaml`
- **Schema:** `global_settings.schema.yaml`
- **Contents:** Machining parameters (spindle speed, feedrates, Z heights, etc.), unit system, theme, and currently selected CNC profile ID

### Tool stock inventory
- **File:** `stock.yaml`
- **Schema:** `stock.schema.yaml`
- **Contents:** List of tools in stock with metadata (id, name, kind, diameter, availability, preference, ATC expected flag, operation counters, manufacturer, SKU/source SKU), with tool families including drill/router/endmill/engraver/v-bit and type-specific attributes (z/table feed, point angle, tip diameter, flute length, min depth, max hits, internal addition precedence)

### Rack configuration
- **File:** `rack.yaml`
- **Schema:** `rack.schema.yaml`
- **Contents:** Rack selection, slot capacity, per-slot tool assignment (`tool_id`) and slot enable/disable state

### CNC profiles
- **Directory:** `cnc_profiles/`
- **Files:** `{profile_name}.yaml` for each profile
- **Schema:** `cnc_profile.schema.yaml`
- **Contents:** Machine-specific settings (fixture plate size, max feed rate, spindle RPMs, spindle delays, ATC slots, origin/scaling, program units, primitive templates under `primitives.*` evaluated through RHAI)
- **Note:** Currently selected profile ID is stored in `global.setting.yaml`

### Fixture profiles
- **Directory:** `fixture_profiles/`
- **Files:** `{profile_name}.yaml` for each profile
- **Schema:** `fixture_profile.schema.yaml`
- **Contents:** Fixture definition (holding method, work origin/reference, locating pins, keep-out zones, occupancy, optional probing/alignment)

### Job profiles
- **Directory:** `job_profiles/`
- **Files:** `{profile_name}.yaml` for each profile
- **Schema:** `job_profile.schema.yaml`
- **Contents:** References to CNC/fixture profiles, default operations/strategies/tool settings, routing/tab defaults, and override policy bounds
