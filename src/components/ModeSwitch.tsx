/**
 * The persona switch (after Affinity's): Cull | Edit | Retouch, centred in the
 * top bar, with hexagonal ends and a lit hexagonal slider that glides to the
 * chosen one. Cull and Retouch are Chris's RapidCulling and RapidRetouch,
 * embedded as modules; Edit is RapidOne's own editor.
 */

export type Mode = "cull" | "edit" | "retouch";

export const MODES: { id: Mode; label: string }[] = [
  { id: "cull", label: "Cull" },
  { id: "edit", label: "Edit" },
  { id: "retouch", label: "Retouch" },
];

export default function ModeSwitch({ mode, onMode }: { mode: Mode; onMode: (m: Mode) => void }) {
  const index = MODES.findIndex((m) => m.id === mode);
  return (
    <div className="modeswitch" role="tablist" aria-label="Mode">
      <span className="modeswitch__slider" style={{ transform: `translateX(${index * 100}%)` }} aria-hidden="true" />
      {MODES.map((m) => (
        <button
          key={m.id}
          type="button"
          role="tab"
          aria-selected={m.id === mode}
          className={`modeswitch__option${m.id === mode ? " modeswitch__option--on" : ""}`}
          onClick={() => onMode(m.id)}
        >
          {m.label}
        </button>
      ))}
    </div>
  );
}
