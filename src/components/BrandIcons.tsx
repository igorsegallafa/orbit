// Brand marks for integration providers.
interface IconProps {
  size?: number;
}

/** Shortcut: rounded square with the "S" loop — simplified brand mark. */
export function ShortcutLogo({ size = 28 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" fill="none">
      <rect width="32" height="32" rx="8" fill="#57B5E1" />
      <path
        d="M20.5 11.2c-1.2-1-3-1.6-4.7-1.6-2.6 0-4.4 1.2-4.4 3 0 1.7 1.4 2.5 3.9 3 2.8.6 4.7 1.3 4.7 3.5 0 2.3-2.1 3.7-5 3.7-2 0-4-.7-5.3-1.9"
        stroke="#fff"
        strokeWidth="2.2"
        strokeLinecap="round"
        fill="none"
      />
    </svg>
  );
}

/** Linear: three descending bars in the brand's purple. */
export function LinearLogo({ size = 28 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" fill="none">
      <rect width="32" height="32" rx="8" fill="#5E6AD2" />
      <path d="M15.4 22.5V9.5h5.1v2.9h-5.1" fill="#fff" />
      <path
        d="M15.4 22.5c-3 0-5.1-1.9-5.1-4.7v-2.9h5.1"
        fill="#fff"
        opacity="0.6"
      />
    </svg>
  );
}

/** Figma: the four-dot brand mark on white. */
export function FigmaLogo({ size = 28 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" fill="none">
      <rect width="32" height="32" rx="8" fill="#fff" />
      <path d="M11.5 24a3.5 3.5 0 0 0 3.5-3.5V17h-3.5a3.5 3.5 0 0 0 0 7z" fill="#0acf83" />
      <path d="M11.5 10a3.5 3.5 0 0 1 3.5 3.5V17h-3.5a3.5 3.5 0 0 1 0-7z" fill="#a259ff" />
      <path d="M15 10h3.5a3.5 3.5 0 0 1 0 7H15V10z" fill="#f7266f" />
      <path d="M15 17h3.5a3.5 3.5 0 0 1 0 7H15v-7z" fill="#ff7262" />
      <path d="M15 3a3.5 3.5 0 0 0-3.5 3.5V10H15a3.5 3.5 0 0 0 0-7z" fill="#1abcfe" />
    </svg>
  );
}