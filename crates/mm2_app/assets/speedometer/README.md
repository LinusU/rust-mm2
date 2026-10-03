# Exterior speedometer

Original generated artwork inspired by the analog HUD instruments in MM3.
No screenshot pixels or game textures are included. `generate.py` rebuilds the
PNG dial and needle with Pillow and the macOS Arial Bold system font (change
the font path on other platforms); playing the game needs neither dependency.

The dial spans 0–300 km/h. Digital speed can exceed that range while the needle
stops at the final tick. The readout uses horizontal velocity magnitude, so
reverse and drifting report ground speed and jumps do not inflate it. Gears
come from the zero-based telemetry gear index, with R for reverse. The RPM bar
uses the car's own engine redline. This is a designed HUD replacement; stock
cockpit instruments retain their authored calibration.
