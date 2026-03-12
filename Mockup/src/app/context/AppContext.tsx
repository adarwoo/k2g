import React, { createContext, useContext, useState, useEffect, ReactNode } from 'react';

export type JobType = 'drill-locating' | 'drill-pth' | 'drill-npth-route';
export type ToolChangeType = 'automatic' | 'manual';
export type ToolStatus = 'in-stock-preferred' | 'in-stock-avoid' | 'out-of-stock' | 'restock' | 'in-rack';
export type UnitSystem = 'mm' | 'in';
export type RackManagement = 'use-existing' | 'create-new';

export interface MachineConfig {
  name: string;
  toolChange: ToolChangeType;
  maxSizeX: number;
  maxSizeY: number;
  maxSpeedXY: number;
  maxSpeedZ: number;
  acceleration: number;
}

export interface Tool {
  id: string;
  type: 'drill' | 'router';
  diameter: number;
  name: string;
  flutes?: number;
  status: ToolStatus;
}

export interface RackSlot {
  slotNumber: number;
  toolId: string | null;
  disabled: boolean;
}

export interface WorkOrigin {
  x: number;
  y: number;
  coordinateSystem: string; // G54, G55, etc.
}

export interface RackOptions {
  management: RackManagement;
  optimizeForFewerChanges: boolean;
  allowMidJobReload: boolean;
}

export interface OperationTemplate {
  id: number;
  name: string;
  template: string;
}

export interface OutputConfig {
  operations: OperationTemplate[];
}

export interface AppConfig {
  jobType: JobType;
  machine: MachineConfig;
  toolStock: Tool[];
  rackConfig: RackSlot[];
  outputConfig: OutputConfig;
  workOrigin: WorkOrigin;
  rackOptions: RackOptions;
}

export interface PCBData {
  fileName: string;
  drillHoles: Array<{ x: number; y: number; diameter: number; type: 'PTH' | 'NPTH' }>;
  routes: Array<{ points: Array<{ x: number; y: number }>; width: number }>;
  boardOutline: Array<{ x: number; y: number }>;
}

interface AppContextType {
  config: AppConfig;
  updateConfig: (updates: Partial<AppConfig>) => void;
  pcbData: PCBData | null;
  setPcbData: (data: PCBData | null) => void;
  gcode: string;
  setGcode: (code: string) => void;
  generateGCode: () => void;
  hiddenTools: Set<string>;
  toggleToolVisibility: (toolId: string) => void;
}

const defaultConfig: AppConfig = {
  jobType: 'drill-pth',
  machine: {
    name: 'CNC Mill 3020',
    toolChange: 'manual',
    maxSizeX: 300,
    maxSizeY: 200,
    maxSpeedXY: 3000,
    maxSpeedZ: 1000,
    acceleration: 500,
  },
  toolStock: [
    { id: 't1', type: 'drill', diameter: 0.8, name: '0.8mm Drill', status: 'in-stock-preferred' },
    { id: 't2', type: 'drill', diameter: 1.0, name: '1.0mm Drill', status: 'in-stock-preferred' },
    { id: 't3', type: 'drill', diameter: 1.5, name: '1.5mm Drill', status: 'in-stock-preferred' },
    { id: 't4', type: 'router', diameter: 2.0, name: '2.0mm End Mill', flutes: 2, status: 'in-stock-preferred' },
    { id: 't5', type: 'router', diameter: 3.175, name: '1/8" End Mill', flutes: 2, status: 'in-stock-preferred' },
  ],
  rackConfig: Array.from({ length: 8 }, (_, i) => ({
    slotNumber: i + 1,
    toolId: i < 5 ? `t${i + 1}` : null,
    disabled: false,
  })),
  outputConfig: {
    operations: [
      { id: 1, name: 'Program Header', template: 'G21 ; Millimeters\nG90 ; Absolute positioning\nG94 ; Feed per minute' },
      { id: 2, name: 'Tool Change', template: 'M6 T{TOOL_NUMBER} ; Change to tool {TOOL_NUMBER}\nM3 S{SPINDLE_SPEED} ; Start spindle' },
      { id: 3, name: 'Drill Operation', template: 'G0 Z{SAFE_Z} ; Move to safe height\nG0 X{X_POS} Y{Y_POS}\nG1 Z{DRILL_DEPTH} F{FEED_RATE}\nG0 Z{SAFE_Z}' },
      { id: 4, name: 'Route Operation', template: 'G0 Z{SAFE_Z}\nG0 X{X_START} Y{Y_START}\nG1 Z{CUT_DEPTH} F{PLUNGE_RATE}\nG1 X{X_END} Y{Y_END} F{FEED_RATE}' },
      { id: 5, name: 'Rapid Move', template: 'G0 X{X_POS} Y{Y_POS} Z{Z_POS}' },
      { id: 6, name: 'Linear Move', template: 'G1 X{X_POS} Y{Y_POS} Z{Z_POS} F{FEED_RATE}' },
      { id: 7, name: 'Arc CW', template: 'G2 X{X_END} Y{Y_END} I{I_OFFSET} J{J_OFFSET} F{FEED_RATE}' },
      { id: 8, name: 'Arc CCW', template: 'G3 X{X_END} Y{Y_END} I{I_OFFSET} J{J_OFFSET} F{FEED_RATE}' },
      { id: 9, name: 'Dwell', template: 'G4 P{DWELL_TIME} ; Dwell in seconds' },
      { id: 10, name: 'Program Footer', template: 'M5 ; Stop spindle\nG0 Z{SAFE_Z}\nG0 X0 Y0 ; Return to home\nM30 ; Program end' },
    ],
  },
  workOrigin: {
    x: 0,
    y: 0,
    coordinateSystem: 'G54',
  },
  rackOptions: {
    management: 'use-existing',
    optimizeForFewerChanges: true,
    allowMidJobReload: false,
  },
};

const AppContext = createContext<AppContextType | undefined>(undefined);

export function AppProvider({ children }: { children: ReactNode }) {
  const [config, setConfig] = useState<AppConfig>(() => {
    const saved = localStorage.getItem('pcb-config');
    if (saved) {
      const parsedConfig = JSON.parse(saved);
      // Merge with default config to ensure all new properties exist
      return {
        ...defaultConfig,
        ...parsedConfig,
        workOrigin: parsedConfig.workOrigin || defaultConfig.workOrigin,
        rackOptions: parsedConfig.rackOptions || defaultConfig.rackOptions,
        machine: { ...defaultConfig.machine, ...parsedConfig.machine },
        outputConfig: parsedConfig.outputConfig || defaultConfig.outputConfig,
      };
    }
    return defaultConfig;
  });

  // Mock PCB data since we're assuming the file is already loaded
  const [pcbData, setPcbData] = useState<PCBData | null>({
    fileName: 'sample_board.gbr',
    drillHoles: Array.from({ length: 20 }, (_, i) => ({
      x: 10 + Math.random() * 80,
      y: 10 + Math.random() * 60,
      diameter: [0.8, 1.0, 1.5][Math.floor(Math.random() * 3)],
      type: Math.random() > 0.3 ? 'PTH' : 'NPTH' as 'PTH' | 'NPTH',
    })),
    routes: [
      {
        points: [
          { x: 5, y: 5 },
          { x: 95, y: 5 },
          { x: 95, y: 75 },
          { x: 5, y: 75 },
          { x: 5, y: 5 },
        ],
        width: 0.2,
      },
    ],
    boardOutline: [
      { x: 0, y: 0 },
      { x: 100, y: 0 },
      { x: 100, y: 80 },
      { x: 0, y: 80 },
    ],
  });
  const [gcode, setGcode] = useState<string>('');
  const [hiddenTools, setHiddenTools] = useState<Set<string>>(new Set());

  useEffect(() => {
    localStorage.setItem('pcb-config', JSON.stringify(config));
  }, [config]);

  useEffect(() => {
    if (pcbData) {
      generateGCode();
    }
  }, [config, pcbData]);

  const updateConfig = (updates: Partial<AppConfig>) => {
    setConfig(prev => ({ ...prev, ...updates }));
  };

  const generateGCode = () => {
    if (!pcbData) {
      setGcode('');
      return;
    }

    const { operations } = config.outputConfig;
    let code = '';

    // Add header
    code += operations[0].template + '\n\n';

    // Generate drill operations based on job type
    if (config.jobType === 'drill-locating' || config.jobType === 'drill-pth' || config.jobType === 'drill-npth-route') {
      const holes = config.jobType === 'drill-pth' 
        ? pcbData.drillHoles.filter(h => h.type === 'PTH')
        : pcbData.drillHoles;

      // Group holes by diameter
      const holesByDiameter = new Map<number, typeof holes>();
      holes.forEach(hole => {
        const existing = holesByDiameter.get(hole.diameter) || [];
        existing.push(hole);
        holesByDiameter.set(hole.diameter, existing);
      });

      // Process each diameter group
      Array.from(holesByDiameter.entries()).forEach(([diameter, holeGroup], idx) => {
        const tool = config.toolStock.find(t => t.type === 'drill' && t.diameter >= diameter);
        if (tool) {
          // Tool change
          const toolChange = operations[1].template
            .replace(/{TOOL_NUMBER}/g, (idx + 1).toString())
            .replace(/{SPINDLE_SPEED}/g, '10000');
          code += `; Tool: ${tool.name} (${tool.diameter}mm)\n${toolChange}\n\n`;

          // Drill each hole
          holeGroup.forEach(hole => {
            const drillOp = operations[2].template
              .replace(/{SAFE_Z}/g, '5.0')
              .replace(/{X_POS}/g, hole.x.toFixed(3))
              .replace(/{Y_POS}/g, hole.y.toFixed(3))
              .replace(/{DRILL_DEPTH}/g, '-2.0')
              .replace(/{FEED_RATE}/g, '100');
            code += drillOp + '\n';
          });
          code += '\n';
        }
      });
    }

    // Generate route operations
    if (config.jobType === 'drill-npth-route' || config.jobType === 'route-board') {
      const routeTool = config.toolStock.find(t => t.type === 'router');
      if (routeTool && pcbData.routes.length > 0) {
        const toolChange = operations[1].template
          .replace(/{TOOL_NUMBER}/g, '99')
          .replace(/{SPINDLE_SPEED}/g, '12000');
        code += `; Tool: ${routeTool.name}\n${toolChange}\n\n`;

        pcbData.routes.forEach(route => {
          if (route.points.length > 1) {
            for (let i = 0; i < route.points.length - 1; i++) {
              const start = route.points[i];
              const end = route.points[i + 1];
              const routeOp = operations[3].template
                .replace(/{SAFE_Z}/g, '5.0')
                .replace(/{X_START}/g, start.x.toFixed(3))
                .replace(/{Y_START}/g, start.y.toFixed(3))
                .replace(/{X_END}/g, end.x.toFixed(3))
                .replace(/{Y_END}/g, end.y.toFixed(3))
                .replace(/{CUT_DEPTH}/g, '-0.2')
                .replace(/{PLUNGE_RATE}/g, '50')
                .replace(/{FEED_RATE}/g, '300');
              code += routeOp + '\n';
            }
          }
        });
        code += '\n';
      }
    }

    // Add footer
    code += operations[9].template.replace(/{SAFE_Z}/g, '5.0') + '\n';

    setGcode(code);
  };

  const toggleToolVisibility = (toolId: string) => {
    setHiddenTools(prev => {
      const newSet = new Set(prev);
      if (newSet.has(toolId)) {
        newSet.delete(toolId);
      } else {
        newSet.add(toolId);
      }
      return newSet;
    });
  };

  return (
    <AppContext.Provider
      value={{
        config,
        updateConfig,
        pcbData,
        setPcbData,
        gcode,
        setGcode,
        generateGCode,
        hiddenTools,
        toggleToolVisibility,
      }}
    >
      {children}
    </AppContext.Provider>
  );
}

export function useApp() {
  const context = useContext(AppContext);
  if (!context) {
    throw new Error('useApp must be used within AppProvider');
  }
  return context;
}

// Variable descriptions for help context
export const VARIABLE_DESCRIPTIONS: Record<string, string> = {
  TOOL_NUMBER: 'The tool number from the rack configuration',
  SPINDLE_SPEED: 'Spindle speed in RPM',
  SAFE_Z: 'Safe Z height for rapid moves (typically 5mm above work surface)',
  X_POS: 'X coordinate position in mm',
  Y_POS: 'Y coordinate position in mm',
  Z_POS: 'Z coordinate position in mm',
  X_START: 'Starting X coordinate for linear/arc moves',
  Y_START: 'Starting Y coordinate for linear/arc moves',
  X_END: 'Ending X coordinate for linear/arc moves',
  Y_END: 'Ending Y coordinate for linear/arc moves',
  DRILL_DEPTH: 'Depth to drill (negative value in mm)',
  CUT_DEPTH: 'Depth of cut for routing (negative value in mm)',
  FEED_RATE: 'Feed rate in mm/min for cutting operations',
  PLUNGE_RATE: 'Feed rate in mm/min for Z-axis plunge',
  I_OFFSET: 'X offset from start point to arc center',
  J_OFFSET: 'Y offset from start point to arc center',
  DWELL_TIME: 'Pause duration in seconds',
};