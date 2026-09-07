# Rustorrent interface

A person checks transfers on a Mac laptop during the day, often alongside other work. Use a quiet light appearance, with a matching dark appearance for evening use.

## Foundations
System font; tabular numerals for transfer data. Restrained blue accent, subtly tinted neutral backgrounds, clear text, and fine separators. Use a compact fixed type scale and predictable spacing.

## Layout
A narrow library sidebar and a flexible transfer list. Put current download and upload rates in the toolbar. Keep session networking details and bandwidth controls in collapsible sections. Each transfer has a name, one clear state, progress, rates, and primary pause/resume and folder actions. Expand to reveal files, trackers, and technical diagnostics.

## Interaction
Use 150–200 ms transitions for feedback only, respect reduced motion, and show a strong focus outline. Dialogs have a visible title, inline validation, keyboard containment, Escape to cancel, and focus restoration. Destructive removal offers one explicit choice about downloaded files.

## Empty and error states
The empty library explains how to add a file or magnet. Filtered lists distinguish no matches from an empty library. Connection interruptions are visible and recover automatically. Peer connection failures belong in diagnostics unless the entire torrent has failed.
