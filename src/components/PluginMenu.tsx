import { useEffect, useRef, useState } from "react";
import { Plug } from "lucide-react";

/**
 * Plugins, beside Teach / apply look: a menu (the Copy menu's look) with Create plugin with
 * agent, coming soon.
 */
export default function PluginMenu({ disabled }: { disabled: boolean }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", down);
    return () => window.removeEventListener("pointerdown", down);
  }, [open]);

  return (
    <div className="topbar__menu pluginmenu" ref={ref}>
      <button type="button" disabled={disabled} onClick={() => setOpen((o) => !o)} title="Plugins" aria-label="Plugins">
        <Plug size={14} /> <span className="topbar__label">Plugins</span>
      </button>
      {open && (
        <div className="topbar__dropdown autostyle__menu copypaste__menu" role="menu">
          <p className="autostyle__heading">Plugins</p>
          {/* Shown at full strength (not greyed out); it does nothing yet. */}
          <button type="button" className="autostyle__train" onClick={() => setOpen(false)} title="Describe a tool and an agent builds it as a plugin (coming soon)">
            Create plugin with agent (coming soon)
          </button>
        </div>
      )}
    </div>
  );
}
