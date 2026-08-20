type Props = {
  size?: "app" | "sm";
};

export function BrandMark({ size = "app" }: Props) {
  return (
    <span className={`brand-mark brand-mark-${size}`} aria-hidden="true">
      <i />
      <i />
    </span>
  );
}
