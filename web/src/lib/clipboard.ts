/** Copy `text` to the system clipboard.
 *
 * Tries the async Clipboard API first (works on `https://` and
 * `localhost`). Falls back to a hidden `<textarea>` + `document.execCommand`
 * when the page is served over plain HTTP from a non-localhost origin —
 * the Clipboard API is gated behind a secure context, so kata served
 * from a LAN box (e.g. `http://home.weakref.org:7878`) would otherwise
 * fail silently. Returns true on success.
 */
export async function copyText(text: string): Promise<boolean> {
  if (window.isSecureContext && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // fall through to the legacy path
    }
  }
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.setAttribute('readonly', '');
  ta.style.position = 'fixed';
  ta.style.top = '-9999px';
  ta.style.left = '-9999px';
  document.body.appendChild(ta);
  ta.focus();
  ta.select();
  let ok = false;
  try {
    ok = document.execCommand('copy');
  } catch {
    ok = false;
  }
  document.body.removeChild(ta);
  return ok;
}
