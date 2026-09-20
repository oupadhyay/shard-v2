# Ambient Stage 1 contract

Stage 1 keeps the real conversation controller and native shortcut path while
making the ambient surface compact and content-led. The dedicated window remains
available as a compatibility path.

## Theme provenance

The palette is mapped from Pierre Dark's authoritative `dark` roles in
[`pierrecomputer/theme/src/palette.ts`](https://github.com/pierrecomputer/theme/blob/71693332d2551a033882216ca7617eb54cdf8aeb/src/palette.ts#L512-L546),
revision `71693332d2551a033882216ca7617eb54cdf8aeb` from September 4, 2026.

The small semantic contract lives in `src/styles.css`:

- surfaces: `--ambient-bg`, `--ambient-surface`, `--ambient-inset`,
  `--ambient-elevated`;
- text and rules: `--ambient-text`, `--ambient-text-secondary`,
  `--ambient-text-muted`, `--ambient-border`, `--ambient-border-strong`;
- interaction: `--ambient-accent`, `--ambient-accent-subtle`,
  `--ambient-on-accent`;
- states: `--ambient-success`, `--ambient-danger`, `--ambient-warning`,
  `--ambient-info`.

The native glass surface uses Pierre's window color at 88% opacity over a
32px blur. Unsupported or reduced transparency uses the solid window color.

## Connected-view mount contract

`src/ui/ambient-view.ts` exports `mountAmbientView`, `closeAmbientView`, and
`isAmbientViewOpen`. `mountAmbientView({ title, render })` supplies
`#ambient-view-content` to one capability renderer. A renderer may return a
cleanup callback. The shell owns the title, back action, Escape behavior, and
focus restoration; capability renderers must not replace the persistent
conversation or composer nodes.

Stable DOM IDs are `ambient-view`, `ambient-view-title`,
`ambient-view-content`, `ambient-view-close`, and `input-field`.

## Remaining validation gaps

- macOS NSPanel focus, vibrancy, global shortcut behavior, Spaces, and display
  changes require validation on macOS;
- live provider streaming, external tools, and OCR-provider calls require
  configured credentials and desktop permissions;
- Stage 2 now connects memory, routines, and history through the mount contract.
  See [AMBIENT_TESTING.md](./AMBIENT_TESTING.md) for the integrated test checklist
  and [TODO.md](./TODO.md) for remaining persistence and platform limits.
