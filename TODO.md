1 - Change the flow
When launched, the application needs to connect to a KiCAD instance which has a PCB available
This implies:
 - Scanning for all KiCad socket (unless passed in environment variable)
 - Checking for PCBs loaded
Unless started from a KiCAD PCB, the user should select from a dropdown list.
This list should be refreshed perdiodically.

=> Exception: If a PCB is already 'opened', the KiCAD instance can go away.
=> If the KiCad instance goes away whilst the data is being collected, the job is dropped, an error is reported (lost connection whilst ..) and the user needs to select from the list.

Once a PCB is selected, the user can configure the job.
A job is created from a Job profile. (the job profile must have been created previously).
The Job profile preconfigures all that can be pre-configured.
The user can override any of the job profile settings in the job.

=> Optimal flow.
1 - Drill PTH for a PCB
User clicks on the Plugin icon.
USer selects a job profile at the top of the screen from a dropdown list.
The last job profile is automatically selected.
The job detail is shown with machining detail.
The user can review the machining, code and rack, and send the GCode to the CNC (if the CNC allows it)

Question: Should locating pins (not fixture holes), which would be 

2 - Collect plating wall thickness and allow override

3 - Does KiCAD support tooling/registration information for drills?
No. Fiducial holes need a special layer

4 - Registration strategy
The datum from KiCAD must include registration holes for PTH PCBs
This must come from the KiCAD data as the films will include fiducials.
A plugin could also create the films, and then add registration holes...