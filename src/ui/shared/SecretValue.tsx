import { useEffect, useRef, useState, type PointerEvent } from "react";
import { beginReveal, endReveal, revealSecret } from "../../lib/ipc";

const MASK = "••••••••";

type Props = {
  serviceId: string;
  headerKey: string;
};

export function SecretValue({ serviceId, headerKey }: Props) {
  const [value, setValue] = useState<string | null>(null);
  const tokenRef = useRef<string | null>(null);
  const generation = useRef(0);

  const hide = () => {
    generation.current += 1;
    const token = tokenRef.current;
    tokenRef.current = null;
    setValue(null);
    if (token) {
      void endReveal(token).catch(() => {
        // Always remask even if the token already expired.
      });
    }
  };

  const show = async (event: PointerEvent<HTMLButtonElement>) => {
    event.preventDefault();
    const gen = generation.current + 1;
    generation.current = gen;
    try {
      const { token } = await beginReveal(serviceId, headerKey);
      if (generation.current !== gen) {
        void endReveal(token).catch(() => undefined);
        return;
      }
      tokenRef.current = token;
      const secret = await revealSecret(serviceId, headerKey, token);
      if (generation.current === gen) setValue(secret);
    } catch {
      if (generation.current === gen) setValue(null);
    }
  };

  useEffect(() => hide, [serviceId, headerKey]); // remask on unmount / header change

  return (
    <button
      type="button"
      className={`secret-mask${value ? " revealed" : ""}`}
      aria-label={value ? "Secret revealed. Release to hide." : "Hold to reveal"}
      onPointerDown={(event) => void show(event)}
      onPointerUp={hide}
      onPointerLeave={hide}
      onPointerCancel={hide}
      onBlur={hide}
    >
      {value ?? MASK}
    </button>
  );
}
