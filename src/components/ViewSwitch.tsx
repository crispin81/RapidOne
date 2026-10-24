/**
 * All | Rated | Unrated, once Organise has sorted a shoot (the same look as
 * the Cull | Edit | Retouch switch; Cull has its own copy): All is the shoot's own folder
 * (rated and unrated together, by capture time), Rated and Unrated its
 * subfolders.
 */

export type FolderView = "all" | "rated" | "unrated";

export interface FolderViews {
  root: string;
  rated: boolean;
  unrated: boolean;
}

const VIEWS: { id: FolderView; label: string }[] = [
  { id: "all", label: "All" },
  { id: "rated", label: "Rated" },
  { id: "unrated", label: "Unrated" },
];

/** Which view `folder` is of the shoot. */
export function viewOf(folder: string | null, views: FolderViews | null): FolderView {
  if (!folder || !views) return "all";
  const name = folder.replace(/[\\/]+$/, "").split(/[\\/]/).pop()?.toLowerCase();
  return name === "rated" ? "rated" : name === "unrated" ? "unrated" : "all";
}

/** The folder to open for a view. */
export function folderFor(view: FolderView, views: FolderViews): string {
  if (view === "all") return views.root;
  const sep = views.root.includes("\\") && !views.root.includes("/") ? "\\" : "/";
  return views.root.replace(/[\\/]+$/, "") + sep + view;
}

export default function ViewSwitch({ view, views, onView }: { view: FolderView; views: FolderViews; onView: (v: FolderView) => void }) {
  const index = VIEWS.findIndex((v) => v.id === view);
  return (
    <div className="viewswitch" role="tablist" aria-label="Photos shown">
      <span className="viewswitch__slider" style={{ transform: `translateX(${index * 100}%)` }} aria-hidden="true" />
      {VIEWS.map((v) => (
        <button
          key={v.id}
          type="button"
          role="tab"
          aria-selected={v.id === view}
          disabled={(v.id === "rated" && !views.rated) || (v.id === "unrated" && !views.unrated)}
          className={`viewswitch__option${v.id === view ? " viewswitch__option--on" : ""}`}
          onClick={() => onView(v.id)}
          title={v.id === "all" ? "All: rated and unrated together, by capture time" : `Only the ${v.id} folder`}
        >
          {v.label}
        </button>
      ))}
    </div>
  );
}
