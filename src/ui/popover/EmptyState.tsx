type Props = {
  onAdd: () => void;
};

export function EmptyState({ onAdd }: Props) {
  return (
    <div className="empty-state">
      <p>Add the HTTP endpoints you own. Pulse will watch them from the tray.</p>
      <p className="empty-hint">
        Unsigned build. Source: github.com/praveensankar969/pulse. If macOS
        blocked Pulse, run this, then open it. Keychain: Always Allow.
      </p>
      <pre className="empty-cmd">xattr -cr /Applications/Pulse.app</pre>
      <button type="button" className="btn primary" onClick={onAdd}>
        Add service
      </button>
    </div>
  );
}
