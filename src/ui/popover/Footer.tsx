type Props = {
  onCheckAll: () => void;
  onAdd: () => void;
  onSettings: () => void;
  onQuit: () => void;
};

export function Footer({ onCheckAll, onAdd, onSettings, onQuit }: Props) {
  return (
    <footer className="popover-foot">
      <div className="pop-foot-l">
        <button type="button" className="mini" onClick={onCheckAll}>
          Check all
        </button>
        <button type="button" className="mini" onClick={onAdd}>
          Add
        </button>
      </div>
      <div className="pop-foot-r">
        <button type="button" className="mini" onClick={onSettings}>
          Settings
        </button>
        <button type="button" className="mini" onClick={onQuit}>
          Quit
        </button>
      </div>
    </footer>
  );
}
