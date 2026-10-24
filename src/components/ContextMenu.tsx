import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

export interface ContextMenuItem {
  label: string;
  icon?: ReactNode;
  /** The question asked before it acts (Yes / No). */
  question: string;
  onConfirm: () => void;
  /** Shown greyed out, can't be chosen. */
  disabled?: boolean;
}

/**
 * A right-click menu at the pointer. Every item asks before it acts: a click
 * shows its question with Yes / No; Yes does it, No closes the menu.
 * Left / right arrows move between Yes and No; Enter answers. Closes on a click
 * elsewhere, Escape, scrolling or the window losing focus.
 */
export default function ContextMenu({
  x,
  y,
  items,
  onClose,
  ask = false,
  header,
}: {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
  /** Open straight on the first item's question (a key did it, not a right-click). */
  ask?: boolean;
  /**
   * Above the items (e.g. stars and pick): given `askThen` to ask a question
   * (Yes / No) before doing something, and `close`.
   */
  header?: (askThen: (question: string, onYes: () => void, onNo?: () => void) => void, close: () => void) => ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [asking, setAsking] = useState<{ question: string; onConfirm: () => void; onCancel?: () => void } | null>(ask && items[0] ? items[0] : null);
  // Closing while a question is open (No, Escape, a click elsewhere) answers No: its
  // onCancel runs (e.g. undoing the change that asked).
  const askingRef = useRef(asking);
  askingRef.current = asking;
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const dismiss = useRef(() => {
    const a = askingRef.current;
    askingRef.current = null;
    a?.onCancel?.();
    onCloseRef.current();
  }).current;
  const [pos, setPos] = useState({ x, y });

  // Kept inside the window.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({ x: Math.max(4, Math.min(x, window.innerWidth - r.width - 4)), y: Math.max(4, Math.min(y, window.innerHeight - r.height - 4)) });
  }, [x, y, asking]);

  useEffect(() => {
    const away = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) dismiss();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        dismiss();
      } else if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && ref.current?.querySelector(".ctxmenu__answers")) {
        // Left / right between Yes and No (and not to the next photo).
        e.preventDefault();
        e.stopPropagation();
        const answers = [...ref.current.querySelectorAll<HTMLButtonElement>(".ctxmenu__answer")];
        const at = answers.indexOf(document.activeElement as HTMLButtonElement);
        const next = at < 0 ? 0 : Math.max(0, Math.min(answers.length - 1, at + (e.key === "ArrowRight" ? 1 : -1)));
        answers[next]?.focus();
      }
    };
    window.addEventListener("mousedown", away, true);
    window.addEventListener("keydown", key, true);
    window.addEventListener("wheel", dismiss, { passive: true });
    window.addEventListener("blur", dismiss);
    return () => {
      window.removeEventListener("mousedown", away, true);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("wheel", dismiss);
      window.removeEventListener("blur", dismiss);
    };
  }, [dismiss]);

  return (
    <div ref={ref} className="ctxmenu" style={{ left: pos.x, top: pos.y }} role="menu" onContextMenu={(e) => e.preventDefault()}>
      {asking ? (
        <div className="ctxmenu__ask">
          <div className="ctxmenu__question">{asking.question}</div>
          <div className="ctxmenu__answers">
            <button
              type="button"
              className="ctxmenu__answer ctxmenu__answer--yes"
              autoFocus
              onClick={() => {
                askingRef.current = null;
                onClose();
                asking.onConfirm();
              }}
            >
              Yes
            </button>
            <button type="button" className="ctxmenu__answer" onClick={dismiss}>
              No
            </button>
          </div>
        </div>
      ) : (
        <>
          {header?.((question, onConfirm, onCancel) => setAsking({ question, onConfirm, onCancel }), onClose)}
          {items.map((item) => (
          <button key={item.label} type="button" role="menuitem" className="ctxmenu__item" disabled={item.disabled} onClick={() => setAsking(item)}>
            {item.icon}
            <span>{item.label}</span>
          </button>
          ))}
        </>
      )}
    </div>
  );
}
