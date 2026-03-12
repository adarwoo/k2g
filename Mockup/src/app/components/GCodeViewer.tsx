import React from 'react';
import { Textarea } from './ui/textarea';

interface GCodeViewerProps {
  code: string;
  onChange?: (code: string) => void;
  readOnly?: boolean;
}

export function GCodeViewer({ code, onChange, readOnly = false }: GCodeViewerProps) {
  const lines = code.split('\n');
  
  return (
    <div className="relative h-full">
      <div className="absolute inset-0 flex">
        {/* Line numbers */}
        <div className="bg-gray-100 px-3 py-4 text-right text-sm text-gray-500 font-mono select-none border-r border-gray-300">
          {lines.map((_, i) => (
            <div key={i} className="leading-6">
              {i + 1}
            </div>
          ))}
        </div>
        
        {/* Code content */}
        <Textarea
          value={code}
          onChange={e => onChange?.(e.target.value)}
          readOnly={readOnly}
          className="flex-1 font-mono text-sm resize-none border-0 rounded-none focus-visible:ring-0"
          style={{ lineHeight: '1.5rem' }}
        />
      </div>
    </div>
  );
}
