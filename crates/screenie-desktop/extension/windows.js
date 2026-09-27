// The windows on screen, for picking one: what GNOME's own screenshot UI offers in its
// window mode, less what isn't an app's window.

import Meta from 'gi://Meta';

const TYPES = new Set([
    Meta.WindowType.NORMAL,
    Meta.WindowType.DIALOG,
    Meta.WindowType.MODAL_DIALOG,
    Meta.WindowType.UTILITY,
]);

/**
 * The windows shown on the active workspace, topmost first, but not those of process
 * `exclude` (screenie's own). Rects are the visible frame, in stage coordinates: what
 * Wayland clients are told the outputs' logical layout is, in either layout mode.
 *
 * @param {number} exclude - a process id
 * @returns {Array} (id, title, app id, [x, y, width, height], focused)
 */
export function shown(exclude) {
    const workspace = global.workspace_manager.get_active_workspace();
    const focus = global.display.focus_window;
    return global.get_window_actors()
        .map(actor => actor.meta_window)
        .filter(w => !w.is_override_redirect() &&
            TYPES.has(w.get_window_type()) &&
            !w.minimized &&
            w.located_on_workspace(workspace) &&
            w.get_monitor() >= 0 &&
            w.get_pid() !== exclude)
        .reverse()
        .map(w => {
            const r = w.get_frame_rect();
            return [
                w.get_id(),
                w.get_title() ?? '',
                w.get_sandboxed_app_id() ?? w.get_wm_class() ?? '',
                [r.x, r.y, r.width, r.height],
                w === focus,
            ];
        });
}
