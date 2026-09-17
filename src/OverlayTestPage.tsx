import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

export default function OverlayTestPage() {
  useEffect(() => {
    [document.documentElement, document.body, document.getElementById('root')]
      .forEach(el => el?.style.setProperty('background', 'transparent', 'important'));
  }, []);

  return (
    <div style={{
      width: '100vw', height: '100vh', boxSizing: 'border-box',
      background: '#00cc55',
      border: '4px solid #00ff88',
      display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center',
      gap: 12, fontFamily: 'sans-serif', color: '#fff',
    }}>
      <div style={{ fontSize: 22, fontWeight: 700, textShadow: '0 2px 6px #000' }}>
        FrameForge Overlay Test
      </div>
      <div style={{ fontSize: 13, opacity: 0.85 }}>If you see green: window + React are working</div>
      <button
        onClick={() => getCurrentWindow().close().catch(() => {})}
        style={{ marginTop: 8, padding: '8px 24px', cursor: 'pointer', fontSize: 14,
          background: '#00ff88', color: '#000', border: 'none', borderRadius: 6, fontWeight: 700 }}
      >
        Close
      </button>
    </div>
  );
}
