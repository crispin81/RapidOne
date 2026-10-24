import type { ReactNode } from "react";
import { Crop, Download, Eraser, Lasso, FolderOpen, House, Image as ImageIcon, LayoutGrid } from "lucide-react";
import { MasksIcon } from "./GradientIcons";
import { PROFILES, userProfileId, userProfileName, type ProfileId, type UserProfile } from "../api";
import ModeSwitch, { type Mode } from "./ModeSwitch";
import PluginMenu from "./PluginMenu";
import { keyLabel } from "../macKeys";

interface Props {
  folder: string | null;
  /** The current photo's profile. */
  profile: ProfileId;
  profileEnabled: boolean;
  /** Adobe's profiles are installed, so Adobe Color is offered. */
  adobeAvailable: boolean;
  /** Profiles the user added (Documents/RapidOne/Profiles), and Add your own profile…. */
  userProfiles: UserProfile[];
  onAddProfiles: () => void;
  /** The current photo's camera, so the menu can say which profiles cover it. */
  camera?: string;
  onOpenFolder: () => void;
  onProfileChange: (profile: ProfileId) => void;
  /** The Presets menu (PresetMenu), placed in the bar. */
  presetMenu: ReactNode;
  /** Lens corrections (LensMenu), placed in the bar. */
  lensMenu: ReactNode;
  /** Auto Style (white balance and exposure learned from the photographer's edits). */
  autoStyleMenu: ReactNode;
  /** Back to the start screen (with a folder open). */
  onHome: () => void;
  /** Cull | Edit | Retouch. The editing tools show only in Edit. */
  mode: Mode;
  /** On the home / opening screen: the bar stays, empty (no modes, no tools). */
  atHome: boolean;
  /** Where Retouch shows its toolbar, either side of the mode switch (its tab only). */
  retouchSlots: { left: (el: HTMLDivElement | null) => void; right: (el: HTMLDivElement | null) => void };
  onMode: (m: Mode) => void;
  /** The crop tool: open (highlighted) and its toggle. */
  cropping: boolean;
  onCrop: () => void;
  /** The Remove tool is open. */
  removing: boolean;
  onRemove: () => void;
  /** Patch (Heal / Clone) open. */
  patching: boolean;
  onPatch: () => void;
  /** The masks (layers panel): open, and its toggle. */
  gradientTool: boolean;
  onGradient: () => void;
  homeEnabled: boolean;
  /** The main pane shows the library (a grid of the photos) rather than the loupe. */
  library: boolean;
  onLibrary: (on: boolean) => void;
  /** Photos picked for export (0: just the open photo), and the Export button, which opens the Export
   *  dialog (there: selected or all shown, format, size, names). */
  selectedCount: number;
  exportEnabled: boolean;
  onExport: () => void;
}

export default function TopBar({
  onHome,
  mode,
  atHome,
  retouchSlots,
  onMode,
  cropping,
  onCrop,
  removing,
  onRemove,
  patching,
  onPatch,
  gradientTool,
  onGradient,
  homeEnabled,
  library,
  onLibrary,
  selectedCount,
  exportEnabled,
  onExport,
  folder,
  profile,
  profileEnabled,
  userProfiles,
  onAddProfiles,
  camera,
  onOpenFolder,
  onProfileChange,
  presetMenu,
  lensMenu,
  autoStyleMenu,
}: Props) {
  // Working folder: in Edit and in Cull, in the same place. The folder's path shows on hover only, to
  // keep the bar clear (the loading bar is in the title bar).
  const folderButton = (
    <button
      type="button"
      onClick={onOpenFolder}
      title={folder ? `Working folder: ${folder}\nClick to choose another` : "Working folder: none open, click to choose one"}
      aria-label="Working folder"
    >
      <FolderOpen size={14} /> <span className="topbar__label">Working folder</span>
    </button>
  );
  return (
    <div className="topbar">
      <div className="topbar__side">
      {mode === "retouch" && !atHome && <div className="topbar__slot" ref={retouchSlots.left} />}
      {mode === "cull" && !atHome && folderButton}
      {mode === "edit" && !atHome && (
      <>
      <button type="button" onClick={onHome} disabled={!homeEnabled} title="Home: back to the start screen" aria-label="Home">
        <House size={14} /> <span className="topbar__label">Home</span>
      </button>
      {folderButton}
      {/* Profile first: every slider works on top of it (brief, section 5b). */}
      <label className="topbar__profile" title={profileHint(profile, userProfiles, camera)}>
        <select
          aria-label="Profile"
          value={shownProfile(profile, userProfiles)}
          disabled={!profileEnabled}
          onChange={(e) => {
            const v = e.currentTarget.value;
            if (v === ADD_PROFILE) onAddProfiles();
            else onProfileChange(v as ProfileId);
          }}
        >
          <optgroup label="Built in">
            {PROFILES.filter((p) => !p.needsAdobe).map((p) => (
              <option key={p.id} value={p.id} title={p.hint}>
                {p.label}
              </option>
            ))}
          </optgroup>
          {userProfiles.length > 0 && (
            <optgroup label="Your profiles">
              {userProfiles.map((u) => {
                const covers = !camera || u.cameras.some((c) => sameCamera(c, camera));
                return (
                  <option key={u.name} value={userProfileId(u.name)} title={coverage(u)}>
                    {u.name}
                    {covers ? "" : " (not for this camera)"}
                  </option>
                );
              })}
            </optgroup>
          )}
          <option value={ADD_PROFILE}>Add your own profile…</option>
        </select>
      </label>
      {/* Lens corrections, beside the camera profile (both are what the photo starts from). */}
      {lensMenu}
      {/* Library (the photos as a grid, in place of the filmstrip) | Loupe (one photo). */}
      <button
        type="button"
        className={library ? "is-on" : undefined}
        onClick={() => onLibrary(true)}
        disabled={!homeEnabled && !library}
        title="Library: the photos as a grid, with stars, picks, filters and All | Rated | Unrated"
        aria-label="Library"
      >
        <LayoutGrid size={14} /> <span className="topbar__label">Library</span>
      </button>
      <button
        type="button"
        className={!library ? "is-on" : undefined}
        onClick={() => onLibrary(false)}
        disabled={!homeEnabled && !library}
        title="Loupe: one photo, with the filmstrip"
        aria-label="Loupe"
      >
        <ImageIcon size={14} /> <span className="topbar__label">Loupe</span>
      </button>
      {/* The loupe's zoom (Fit, and the percentage to pick from), drawn here by the Viewer. */}
      <span className="topbar__zoom" id="topbar-zoom" />
      </>
      )}
      </div>
      {!atHome && (
        <div className="topbar__center">
          <ModeSwitch mode={mode} onMode={onMode} />
          {/* Retouch's status line (starting the engine, progress, errors), beside the switch: no footer under the photo. */}
        </div>
      )}
      <div className="topbar__side topbar__side--right">
      {mode === "retouch" && !atHome && <div className="topbar__slot" ref={retouchSlots.right} />}
      {mode === "edit" && !atHome && (
      <>
      {/* Editing tools (moved up from the old strip left of the viewer). */}
      <button
        type="button"
        className={cropping ? "is-on" : undefined}
        onClick={onCrop}
        disabled={!homeEnabled && !cropping}
        title="Crop and straighten (R, or Shift+R from any panel; Enter or Esc: done)"
        aria-label="Crop"
      >
        <Crop size={14} />
      </button>
      <button
        type="button"
        className={removing ? "is-on" : undefined}
        onClick={onRemove}
        disabled={!homeEnabled && !removing}
        title={keyLabel("Erase (E): drag a box round an object, say what it is (optional), Enter; B: brush instead. Ctrl+E: remove by words. E or Esc: done")}
        aria-label="Remove"
      >
        <Eraser size={14} /> <span className="topbar__label">Remove</span>
      </button>
      <button
        type="button"
        className={patching ? "is-on" : undefined}
        onClick={onPatch}
        disabled={!homeEnabled && !patching}
        title="Patch (Q): draw round what to fix, drag it onto clean texture. Heal (texture, the tone from round it) or Clone (an exact copy). Q or Esc: done"
        aria-label="Patch"
      >
        <Lasso size={14} /> <span className="topbar__label">Patch</span>
      </button>
      {/* The masking tools, a group of their own with space either side. */}
      <span className="topbar__group topbar__group--masking">
        <button
          type="button"
          className={gradientTool ? "is-on" : undefined}
          onClick={onGradient}
          disabled={!homeEnabled && !gradientTool}
          title="Masks (M): pick a type (Subject, Brush, Linear, Radial) to start a layer, then + / − to add or take away shapes on it, and set its sliders below (M or Esc: done)"
          aria-label="Masks"
        >
          <MasksIcon size={14} /> <span className="topbar__label">Masks</span>
        </button>
      </span>
      {presetMenu}
      {autoStyleMenu}
      {/* Plugins, with a gap either side. */}
      <PluginMenu disabled={!homeEnabled} />
      {/* Export, last: one button, with its name showing. Which photos (selected or all shown), format, size and names are chosen in the dialog it opens. */}
      <button
        type="button"
        className="topbar__export"
        onClick={onExport}
        disabled={!exportEnabled}
        title={
          selectedCount > 1
            ? keyLabel(`Export photos: ${selectedCount} are selected (Ctrl-click / Shift-click in the filmstrip or library); choose which in the dialog`)
            : keyLabel("Export photos: the open photo, or all those shown (choose in the dialog). Ctrl-click / Shift-click photos to select several")
        }
        aria-label="Export"
      >
        <Download size={14} /> <span>Export</span>
      </button>
      </>
      )}
      </div>
    </div>
  );
}


/** The menu entry that opens the file dialog rather than choosing a profile. */
const ADD_PROFILE = "__add-profile__";

const squash = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, "");
const sameCamera = (a: string, b: string) => squash(a) === squash(b);
const coverage = (u: UserProfile) => (u.cameras.length ? `For ${u.cameras.join(", ")}` : "No camera added for it yet");

/** What the menu shows as chosen: a user profile under its own entry. */
function shownProfile(profile: ProfileId, users: UserProfile[]): string {
  const name = userProfileName(profile);
  if (name && users.some((u) => u.name === name)) return userProfileId(name);
  // An Adobe id without the user's profile: it renders as its Shinobi match.
  if (profile === "adobeColor") return "shinobiColour";
  if (profile === "adobeStandard") return "shinobiStandard";
  return profile;
}

function profileHint(profile: ProfileId, users: UserProfile[], camera?: string): string | undefined {
  const name = userProfileName(profile);
  if (!name) return PROFILES.find((p) => p.id === profile)?.hint;
  const u = users.find((x) => x.name === name);
  if (!u) return `${name} isn't in your profiles: Shinobi is used (Add your own profile…)`;
  const covers = !camera || u.cameras.some((c) => sameCamera(c, camera));
  return covers ? `${name}: your profile (${coverage(u)})` : `${name} doesn't cover ${camera}: Shinobi is used for it`;
}
