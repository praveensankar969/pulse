type Props = {
  onCheckAll: () => void;
  onSettings: () => void;
  onQuit: () => void;
};

export function Footer({ onCheckAll, onSettings, onQuit }: Props) {
  return (
    <footer className="popover-foot">
      <button type="button" className="text-btn" onClick={onCheckAll}>
        Check all
      </button>
      <span className="foot-spacer" />
      <button type="button" className="text-btn" onClick={onSettings}>
        Settings
      </button>
      <button type="button" className="text-btn danger-quiet" onClick={onQuit}>
        Quit
      </button>
    </footer>
  );
}
