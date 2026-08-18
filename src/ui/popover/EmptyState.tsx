type Props = {
  onAdd: () => void;
};

export function EmptyState({ onAdd }: Props) {
  return (
    <div className="empty-state">
      <p>Add the HTTP endpoints you own. Pulse will watch them from the tray.</p>
      <p className="empty-hint">
        macOS will ask Pulse to use the keychain — choose Always Allow.
      </p>
      <button type="button" className="btn primary" onClick={onAdd}>
        Add service
      </button>
    </div>
  );
}
