## Add context object
The context (or ctx) is a singleton instance, accessible to the UI and rhai-parser which stores the following:
 - The cli arguments
 - The board data
 - The stiched board data
 - The catalogs
 - The stock
 - The CNCs
 - The process profiles
 - The current project
 - Regeneration status
 - KiCad status
 - Errors and warning
    - Each error/warning have a domain (generator, stock, cnc profile etc.)
    - A domain can be cleaned (generator)
 - Status and information
    - Use a dictonnary to hold system status (use code constants for keys)
 - Tools mapping:

## Startup
 - We need to get all persisted data loaded up
 - We need to add a module for the GCode generation
    - Takes the context in
    - Generates the code
    - Report an errors during the generation
    - Reports warning

It is intended to provide all relevant information about the session.

## Replace the ATC tab with tooling tab
 - The page lists all tooling reqs (drills and route) and their matching views
 - The view shows the choices made during the generation
 - The view is split with the board on the left and the tool list and mapping on the right
 - The left is holes / PTH holes / Internal routes / External routes -> Stock tools used [Could be many drill + route] -> Target size (to account for plating) -> Tools used + [ATC slot]
 - Any tool changes from the rack must be clearly shown. So racks may have some spare slots which require manual changes.

## Generator module
 - This is where the GCode gets created.
 - This instance must support async operations such that is can be cancelled at any time
 - Multiple instance must be allowed to run concurrently
 - The generator takes the context in

## Rack profile

 - The rack profile allows the user to create a custom tool list which can be used in preference
 - The list of tools are mappedd is T1 to Tx
 - This list can be used for ATC machines or manual machines
 - It allow the user to create some standard tools set
 - For ATC operation, it allows having a fixed set of tools preloaded in the CNC, and avoid re-loading the rack
 - Each tool can hold:
   - a fixed tool (chosen from the stock)
   - a do not use
   - a spare (so more tools can be added)
 - The rack profile is not limited in size. If the CNC has fewer slot, a warning is generated.
 - For the ATC, the profile defines the behaviour during generation
   - Single rack only : The generator will try to only use the given rack if possible. An error is generated otherwise
   - Allow reload rack : If the rack cannot hold all the tools, prompt the user to reload the rack during machining
   - Allow Manual changes : Once the rack is fully used, the program will prompt the user to manually change the bits.
      - The last bit from the rack is put back in the rack, and the user is prompted to load a new tool by hand

 - The rack profile can be added to a process profile [as one, a list, ANY, New]
    - Single selection: The project is forced to use a pre-defined rack. The rack can be made of all spares - which is the same as 'New'
    - list: The project gets a default rack, but can choose from a list. New can be part of the list.
    - New: A new rack is generated for every projects
    - Any: The user can choose from any of the racks and 'new'
    - For List and Any, a default value is required. The default value must belong to the list unless it is new.
    - Example : Rack profiles:
      - [Standard sizes metric medium; New] : Single rack. The user can only overwrite from standard or new
      - [New] : A new rack

 Example: A user creates s

1 - Change the flow
When launched, the application needs to connect to a KiCAD instance which has a PCB available
This implies:
 - Scanning for all KiCad socket (unless passed in environment variable)
 - Checking for PCBs loaded
Unless started from a KiCAD PCB, the user should select from a dropdown list.
This list should be refreshed perdiodically.

=> Exception: If a PCB is already 'opened', the KiCAD instance can go away.
=> If the KiCad instance goes away whilst the data is being collected, the project is dropped, an error is reported (lost connection whilst ..) and the user needs to select from the list.

Once a PCB is selected, the user can configure the project.
A project is created from a Process profile. (the process profile must have been created previously).
The Process profile preconfigures all that can be pre-configured.
The user can override any of the process profile settings in the project.

=> Optimal flow.
1 - Drill PTH for a PCB
User clicks on the Plugin icon.
USer selects a process profile at the top of the screen from a dropdown list.
The last process profile is automatically selected.
The project detail is shown with machining detail.
The user can review the machining, code and rack, and send the GCode to the CNC (if the CNC allows it)

Question: Should locating pins (not fixture holes), which would be

2 - Collect plating wall thickness and allow override

3 - Does KiCAD support tooling/registration information for drills?
No. Fiducial holes need a special layer

4 - Registration strategy
The datum from KiCAD must include registration holes for PTH PCBs
This must come from the KiCAD data as the films will include fiducials.
A plugin could also create the films, and then add registration holes...

# Project

Once a project is finalized, it can be saved. Saving it preserves all the data and remove
the need for KiCAD connection to re-access the same GCode etc.
Note: The GCode is also saved and reloaded.
Once loaded, no generation takes place because the entire generated data was loaded.
This is to guarantee the data is the same.
As soon as the user start making changes to the project, the data is regenerated, and
the project can be saved again.

## Open project
It is possible to open a project which loads a project file complete with all profiles data.

## Saving project
It is possible to save the project data. Once save, the project name in place of the KiCAD pcb name

# Command line interface

Adding a <filename> to the cli which is a saved project.
When opening a project file, the top bar shows the file.
