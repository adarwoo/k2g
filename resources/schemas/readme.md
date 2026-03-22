## Schema files

The schema directory serves 2 purposes:
 1. It create the schema to validate the various configuration files
 2. It specifies the default values which, in turn, can be used to create a default file

Therefore it is important to properly document all the nodes, as the documentation
will allow generating the comments of the generated files.

All schema files must end with **_schema.yaml**.

## Persistence strategy

Configuration and state are persisted in the user's config directory:

### Global configuration
- **File:** `global.setting.yaml`
- **Schema:** `global_settings.schema.yaml`
- **Contents:** Machining parameters (spindle speed, feedrates, Z heights, etc.), unit system, theme, and currently selected CNC profile ID

### Tool stock inventory
- **File:** `stock.yaml`
- **Schema:** `stock.schema.yaml`
- **Contents:** List of tools in stock with all metadata (id, name, kind, diameter, feed rate, spindle RPM, status, operation count, manufacturer, SKU)

### CNC profiles
- **Directory:** `cnc_profiles/`
- **Files:** `{profile_name}.yaml` for each profile
- **Schema:** `cnc_profile.schema.yaml`
- **Contents:** Machine-specific settings (fixture plate size, max feed rate, spindle RPMs, ATC slots, G-code templates for header, footer, drill/route/tool-change cycles)
- **Note:** Currently selected profile ID is stored in `global.setting.yaml`
