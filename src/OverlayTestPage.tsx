import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

export default function OverlayTestPage() {
  useEffect(() => {
    [document.documentElement, document.body, document.getElementById('root')]
      .forEach(el => el?.style.setProperty('background', 'transparent', 'important'));
  }, []);

  return (
    <div className="box-border flex h-screen w-screen flex-col items-center justify-center gap-3 border-4 border-[#00ff88] bg-[#00cc55] font-sans text-white">
      <div className="text-[22px] font-bold drop-shadow-[0_2px_6px_#000]">
        FrameForge Overlay Test
      </div>
      <div className="text-[13px] opacity-85">If you see green: window + React are working</div>
      <button
        onClick={() => getCurrentWindow().close().catch(() => {})}
        className="mt-2 cursor-pointer rounded-md border-0 bg-[#00ff88] px-6 py-2 text-sm font-bold text-black"
      >
        Close
      </button>
    </div>
  );
}
