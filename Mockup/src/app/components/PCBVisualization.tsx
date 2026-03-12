import React, { useEffect, useRef } from 'react';
import { PCBData, Tool } from '../context/AppContext';

interface PCBVisualizationProps {
  data: PCBData;
  tools: Tool[];
  hiddenTools: Set<string>;
  width?: number;
  height?: number;
}

export function PCBVisualization({ 
  data, 
  tools, 
  hiddenTools,
  width = 600, 
  height = 400 
}: PCBVisualizationProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    // Clear canvas
    ctx.fillStyle = '#1a1a1a';
    ctx.fillRect(0, 0, width, height);

    // Calculate bounds
    const allX = [
      ...data.drillHoles.map(h => h.x),
      ...data.boardOutline.map(p => p.x),
      ...data.routes.flatMap(r => r.points.map(p => p.x)),
    ];
    const allY = [
      ...data.drillHoles.map(h => h.y),
      ...data.boardOutline.map(p => p.y),
      ...data.routes.flatMap(r => r.points.map(p => p.y)),
    ];

    const minX = Math.min(...allX, 0);
    const maxX = Math.max(...allX, 100);
    const minY = Math.min(...allY, 0);
    const maxY = Math.max(...allY, 100);

    const rangeX = maxX - minX;
    const rangeY = maxY - minY;
    const padding = 40;

    // Scale to fit canvas with padding
    const scaleX = (width - 2 * padding) / rangeX;
    const scaleY = (height - 2 * padding) / rangeY;
    const scale = Math.min(scaleX, scaleY);

    const toCanvasX = (x: number) => padding + (x - minX) * scale;
    const toCanvasY = (y: number) => height - padding - (y - minY) * scale;

    // Draw grid
    ctx.strokeStyle = '#333';
    ctx.lineWidth = 0.5;
    for (let i = 0; i <= 10; i++) {
      const x = padding + (i / 10) * (width - 2 * padding);
      const y = padding + (i / 10) * (height - 2 * padding);
      ctx.beginPath();
      ctx.moveTo(x, padding);
      ctx.lineTo(x, height - padding);
      ctx.stroke();
      ctx.beginPath();
      ctx.moveTo(padding, y);
      ctx.lineTo(width - padding, y);
      ctx.stroke();
    }

    // Draw board outline
    if (data.boardOutline.length > 0) {
      ctx.strokeStyle = '#4ade80';
      ctx.lineWidth = 2;
      ctx.beginPath();
      data.boardOutline.forEach((point, i) => {
        const x = toCanvasX(point.x);
        const y = toCanvasY(point.y);
        if (i === 0) {
          ctx.moveTo(x, y);
        } else {
          ctx.lineTo(x, y);
        }
      });
      ctx.closePath();
      ctx.stroke();
    }

    // Draw routes
    data.routes.forEach(route => {
      ctx.strokeStyle = '#fbbf24';
      ctx.lineWidth = 2;
      ctx.beginPath();
      route.points.forEach((point, i) => {
        const x = toCanvasX(point.x);
        const y = toCanvasY(point.y);
        if (i === 0) {
          ctx.moveTo(x, y);
        } else {
          ctx.lineTo(x, y);
        }
      });
      ctx.stroke();
    });

    // Draw drill holes
    data.drillHoles.forEach(hole => {
      const tool = tools.find(t => t.type === 'drill' && t.diameter >= hole.diameter);
      if (tool && !hiddenTools.has(tool.id)) {
        const x = toCanvasX(hole.x);
        const y = toCanvasY(hole.y);
        const radius = (hole.diameter / 2) * scale;

        // PTH holes are blue, NPTH are red
        ctx.fillStyle = hole.type === 'PTH' ? '#3b82f6' : '#ef4444';
        ctx.beginPath();
        ctx.arc(x, y, Math.max(radius, 2), 0, 2 * Math.PI);
        ctx.fill();

        // Draw crosshair
        ctx.strokeStyle = '#fff';
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(x - 4, y);
        ctx.lineTo(x + 4, y);
        ctx.moveTo(x, y - 4);
        ctx.lineTo(x, y + 4);
        ctx.stroke();
      }
    });

    // Draw legend
    ctx.fillStyle = '#fff';
    ctx.font = '12px monospace';
    ctx.fillText(`Board: ${rangeX.toFixed(1)} x ${rangeY.toFixed(1)} mm`, padding, 20);
    ctx.fillText(`Holes: ${data.drillHoles.length}`, padding, 35);

  }, [data, tools, hiddenTools, width, height]);

  return (
    <canvas 
      ref={canvasRef} 
      width={width} 
      height={height}
      className="border border-gray-300 rounded-lg bg-gray-900"
    />
  );
}
