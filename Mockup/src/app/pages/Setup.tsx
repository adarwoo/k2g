import React, { useState } from 'react';
import { Layout } from '../components/Layout';
import { useApp, VARIABLE_DESCRIPTIONS } from '../context/AppContext';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../components/ui/tabs';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../components/ui/card';
import { Label } from '../components/ui/label';
import { Input } from '../components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import { Button } from '../components/ui/button';
import { Textarea } from '../components/ui/textarea';
import { Checkbox } from '../components/ui/checkbox';
import { RadioGroup, RadioGroupItem } from '../components/ui/radio-group';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '../components/ui/tooltip';
import { Plus, Trash2, HelpCircle } from 'lucide-react';
import { toast } from 'sonner';
import type { Tool, RackSlot, OperationTemplate, ToolStatus, UnitSystem } from '../context/AppContext';

export function Setup() {
  const { config, updateConfig } = useApp();
  const [activeTab, setActiveTab] = useState('job');
  const [unitSystem, setUnitSystem] = useState<UnitSystem>('mm');

  const convertToDisplay = (value: number): number => {
    if (unitSystem === 'in') {
      return value / 25.4; // mm to inches
    }
    return value;
  };

  const convertFromDisplay = (value: number): number => {
    if (unitSystem === 'in') {
      return value * 25.4; // inches to mm
    }
    return value;
  };

  const formatValue = (value: number): string => {
    return convertToDisplay(value).toFixed(unitSystem === 'in' ? 4 : 2);
  };

  const updateJobType = (jobType: string) => {
    updateConfig({ jobType: jobType as any });
    toast.success('Job type updated');
  };

  const updateWorkOrigin = (field: string, value: any) => {
    updateConfig({
      workOrigin: { ...config.workOrigin, [field]: value },
    });
  };

  const updateRackOptions = (field: string, value: any) => {
    updateConfig({
      rackOptions: { ...config.rackOptions, [field]: value },
    });
  };

  const updateMachine = (field: string, value: any) => {
    updateConfig({
      machine: { ...config.machine, [field]: value },
    });
  };

  const addTool = () => {
    const newTool: Tool = {
      id: `t${Date.now()}`,
      type: 'drill',
      diameter: 1.0,
      name: 'New Tool',
      status: 'in-stock-preferred',
    };
    updateConfig({ toolStock: [...config.toolStock, newTool] });
    toast.success('Tool added');
  };

  const updateTool = (id: string, updates: Partial<Tool>) => {
    updateConfig({
      toolStock: config.toolStock.map(t => (t.id === id ? { ...t, ...updates } : t)),
    });
  };

  const deleteTool = (id: string) => {
    updateConfig({
      toolStock: config.toolStock.filter(t => t.id !== id),
      rackConfig: config.rackConfig.map(slot => 
        slot.toolId === id ? { ...slot, toolId: null } : slot
      ),
    });
    toast.success('Tool deleted');
  };

  const updateRackSlot = (slotNumber: number, toolId: string | null) => {
    updateConfig({
      rackConfig: config.rackConfig.map(slot =>
        slot.slotNumber === slotNumber ? { ...slot, toolId } : slot
      ),
    });
  };

  const updateRackSlotDisabled = (slotNumber: number, disabled: boolean) => {
    updateConfig({
      rackConfig: config.rackConfig.map(slot =>
        slot.slotNumber === slotNumber ? { ...slot, disabled, toolId: disabled ? null : slot.toolId } : slot
      ),
    });
  };

  const updateOperation = (id: number, template: string) => {
    updateConfig({
      outputConfig: {
        operations: config.outputConfig.operations.map(op =>
          op.id === id ? { ...op, template } : op
        ),
      },
    });
  };

  const insertVariable = (operationId: number, variable: string) => {
    const operation = config.outputConfig.operations.find(op => op.id === operationId);
    if (operation) {
      updateOperation(operationId, operation.template + `{${variable}}`);
    }
  };

  const getToolStatus = (tool: Tool): ToolStatus => {
    // Check if tool is in rack
    const isInRack = config.rackConfig.some(slot => slot.toolId === tool.id && !slot.disabled);
    if (isInRack) {
      return 'in-rack';
    }
    return tool.status;
  };

  const toolStatusLabels: Record<ToolStatus, string> = {
    'in-stock-preferred': 'In Stock, Preferred',
    'in-stock-avoid': 'In Stock, Avoid',
    'out-of-stock': 'Out of Stock',
    'restock': 'Restock',
    'in-rack': 'In Rack',
  };

  return (
    <Layout>
      <div className="p-8">
        <div className="max-w-6xl mx-auto">
          <div className="mb-8">
            <h2 className="text-2xl font-semibold mb-2">Setup</h2>
            <p className="text-gray-600">
              Configure job parameters and machine settings
            </p>
          </div>

          <Tabs value={activeTab} onValueChange={setActiveTab}>
            <TabsList className="grid w-full grid-cols-5">
              <TabsTrigger value="job">Job</TabsTrigger>
              <TabsTrigger value="machine">Machine</TabsTrigger>
              <TabsTrigger value="tools">Tool Stock</TabsTrigger>
              <TabsTrigger value="rack">Rack Config</TabsTrigger>
              <TabsTrigger value="output">Output</TabsTrigger>
            </TabsList>

            {/* Job Configuration */}
            <TabsContent value="job" className="space-y-4">
              <Card>
                <CardHeader>
                  <CardTitle>Job Type</CardTitle>
                  <CardDescription>
                    Select the manufacturing operation
                  </CardDescription>
                </CardHeader>
                <CardContent className="space-y-4">
                  <RadioGroup value={config.jobType} onValueChange={updateJobType}>
                    <div className="space-y-3">
                      <div className="flex items-center space-x-3">
                        <RadioGroupItem value="drill-locating" id="drill-locating" />
                        <Label htmlFor="drill-locating" className="cursor-pointer font-normal">
                          Drill Locating Pins
                        </Label>
                      </div>
                      <div className="flex items-center space-x-3">
                        <RadioGroupItem value="drill-pth" id="drill-pth" />
                        <Label htmlFor="drill-pth" className="cursor-pointer font-normal">
                          Drill PTH Holes
                        </Label>
                      </div>
                      <div className="flex items-center space-x-3">
                        <RadioGroupItem value="drill-npth-route" id="drill-npth-route" />
                        <Label htmlFor="drill-npth-route" className="cursor-pointer font-normal">
                          Drill NPTH and Route
                        </Label>
                      </div>
                    </div>
                  </RadioGroup>
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle>Work Origin</CardTitle>
                  <CardDescription>
                    Set the G5x coordinate system origin
                  </CardDescription>
                </CardHeader>
                <CardContent className="space-y-4">
                  <div className="grid grid-cols-3 gap-4">
                    <div className="space-y-2">
                      <Label>Coordinate System</Label>
                      <Select
                        value={config.workOrigin.coordinateSystem}
                        onValueChange={v => updateWorkOrigin('coordinateSystem', v)}
                      >
                        <SelectTrigger>
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="G54">G54</SelectItem>
                          <SelectItem value="G55">G55</SelectItem>
                          <SelectItem value="G56">G56</SelectItem>
                          <SelectItem value="G57">G57</SelectItem>
                          <SelectItem value="G58">G58</SelectItem>
                          <SelectItem value="G59">G59</SelectItem>
                        </SelectContent>
                      </Select>
                    </div>

                    <div className="space-y-2">
                      <Label>X Origin (mm)</Label>
                      <Input
                        type="number"
                        value={config.workOrigin.x}
                        onChange={e => updateWorkOrigin('x', parseFloat(e.target.value))}
                      />
                    </div>

                    <div className="space-y-2">
                      <Label>Y Origin (mm)</Label>
                      <Input
                        type="number"
                        value={config.workOrigin.y}
                        onChange={e => updateWorkOrigin('y', parseFloat(e.target.value))}
                      />
                    </div>
                  </div>
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle>Rack Management (ATC)</CardTitle>
                  <CardDescription>
                    Configure automatic tool change behavior
                  </CardDescription>
                </CardHeader>
                <CardContent className="space-y-4">
                  <div className="space-y-2">
                    <Label>Rack Configuration</Label>
                    <RadioGroup
                      value={config.rackOptions.management}
                      onValueChange={v => updateRackOptions('management', v)}
                    >
                      <div className="flex items-center space-x-3">
                        <RadioGroupItem value="use-existing" id="use-existing" />
                        <Label htmlFor="use-existing" className="cursor-pointer font-normal">
                          Use Existing Rack (no changes)
                        </Label>
                      </div>
                      <div className="flex items-center space-x-3">
                        <RadioGroupItem value="create-new" id="create-new" />
                        <Label htmlFor="create-new" className="cursor-pointer font-normal">
                          Create a New Rack
                        </Label>
                      </div>
                    </RadioGroup>
                  </div>

                  <div className="space-y-3 pt-4 border-t">
                    <div className="flex items-center space-x-3">
                      <Checkbox
                        id="optimize"
                        checked={config.rackOptions.optimizeForFewerChanges}
                        onCheckedChange={v => updateRackOptions('optimizeForFewerChanges', v)}
                      />
                      <Label htmlFor="optimize" className="cursor-pointer font-normal">
                        Optimize the rack for fewer changes
                      </Label>
                    </div>

                    <div className="flex items-center space-x-3">
                      <Checkbox
                        id="reload"
                        checked={config.rackOptions.allowMidJobReload}
                        onCheckedChange={v => updateRackOptions('allowMidJobReload', v)}
                      />
                      <Label htmlFor="reload" className="cursor-pointer font-normal">
                        Allow reloading the rack mid-job
                      </Label>
                    </div>
                  </div>
                </CardContent>
              </Card>
            </TabsContent>

            {/* Machine Configuration */}
            <TabsContent value="machine" className="space-y-4">
              <Card>
                <CardHeader>
                  <CardTitle>CNC Machine Characteristics</CardTitle>
                  <CardDescription>
                    Define your machine's capabilities
                  </CardDescription>
                </CardHeader>
                <CardContent className="space-y-4">
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                    <div className="space-y-2">
                      <Label>Machine Name</Label>
                      <Input
                        value={config.machine.name}
                        onChange={e => updateMachine('name', e.target.value)}
                      />
                    </div>

                    <div className="space-y-2">
                      <Label>Tool Change Type</Label>
                      <Select
                        value={config.machine.toolChange}
                        onValueChange={v => updateMachine('toolChange', v)}
                      >
                        <SelectTrigger>
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="automatic">Automatic Tool Change</SelectItem>
                          <SelectItem value="manual">Manual Tool Change</SelectItem>
                        </SelectContent>
                      </Select>
                    </div>

                    <div className="space-y-2">
                      <Label>Maximum Size X (mm)</Label>
                      <Input
                        type="number"
                        value={config.machine.maxSizeX}
                        onChange={e => updateMachine('maxSizeX', parseFloat(e.target.value))}
                      />
                    </div>

                    <div className="space-y-2">
                      <Label>Maximum Size Y (mm)</Label>
                      <Input
                        type="number"
                        value={config.machine.maxSizeY}
                        onChange={e => updateMachine('maxSizeY', parseFloat(e.target.value))}
                      />
                    </div>

                    <div className="space-y-2">
                      <Label>Max Speed XY (mm/min)</Label>
                      <Input
                        type="number"
                        value={config.machine.maxSpeedXY}
                        onChange={e => updateMachine('maxSpeedXY', parseFloat(e.target.value))}
                      />
                    </div>

                    <div className="space-y-2">
                      <Label>Max Speed Z (mm/min)</Label>
                      <Input
                        type="number"
                        value={config.machine.maxSpeedZ}
                        onChange={e => updateMachine('maxSpeedZ', parseFloat(e.target.value))}
                      />
                    </div>

                    <div className="space-y-2">
                      <Label>Acceleration (mm/s²)</Label>
                      <Input
                        type="number"
                        value={config.machine.acceleration}
                        onChange={e => updateMachine('acceleration', parseFloat(e.target.value))}
                      />
                    </div>
                  </div>
                </CardContent>
              </Card>
            </TabsContent>

            {/* Tool Stock Configuration */}
            <TabsContent value="tools" className="space-y-4">
              <Card>
                <CardHeader>
                  <div className="flex items-center justify-between">
                    <div>
                      <CardTitle>Tool Stock</CardTitle>
                      <CardDescription>
                        Manage available drill bits and router bits
                      </CardDescription>
                    </div>
                    <Select value={unitSystem} onValueChange={(v: UnitSystem) => setUnitSystem(v)}>
                      <SelectTrigger className="w-24">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="mm">mm</SelectItem>
                        <SelectItem value="in">in</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                </CardHeader>
                <CardContent>
                  <div className="space-y-4">
                    {config.toolStock.map(tool => {
                      const currentStatus = getToolStatus(tool);
                      const isInRack = currentStatus === 'in-rack';
                      
                      return (
                        <div key={tool.id} className="flex gap-4 items-end p-4 border rounded-lg">
                          <div className="flex-1 grid grid-cols-5 gap-4">
                            <div className="space-y-2">
                              <Label>Name</Label>
                              <Input
                                value={tool.name}
                                onChange={e => updateTool(tool.id, { name: e.target.value })}
                              />
                            </div>

                            <div className="space-y-2">
                              <Label>Type</Label>
                              <Select
                                value={tool.type}
                                onValueChange={v => updateTool(tool.id, { type: v as any })}
                              >
                                <SelectTrigger>
                                  <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                  <SelectItem value="drill">Drill Bit</SelectItem>
                                  <SelectItem value="router">Router Bit</SelectItem>
                                </SelectContent>
                              </Select>
                            </div>

                            <div className="space-y-2">
                              <Label>Diameter ({unitSystem})</Label>
                              <Input
                                type="number"
                                step={unitSystem === 'in' ? '0.0001' : '0.01'}
                                value={formatValue(tool.diameter)}
                                onChange={e =>
                                  updateTool(tool.id, { diameter: convertFromDisplay(parseFloat(e.target.value)) })
                                }
                              />
                            </div>

                            {tool.type === 'router' && (
                              <div className="space-y-2">
                                <Label>Flutes</Label>
                                <Input
                                  type="number"
                                  value={tool.flutes || 2}
                                  onChange={e =>
                                    updateTool(tool.id, { flutes: parseInt(e.target.value) })
                                  }
                                />
                              </div>
                            )}

                            <div className="space-y-2">
                              <Label>Status</Label>
                              <Select
                                value={currentStatus}
                                onValueChange={v => updateTool(tool.id, { status: v as ToolStatus })}
                                disabled={isInRack}
                              >
                                <SelectTrigger className={isInRack ? 'text-gray-500' : ''}>
                                  <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                  {Object.entries(toolStatusLabels).map(([value, label]) => (
                                    <SelectItem 
                                      key={value} 
                                      value={value}
                                      disabled={value === 'in-rack'}
                                    >
                                      {label}
                                    </SelectItem>
                                  ))}
                                </SelectContent>
                              </Select>
                            </div>
                          </div>

                          <Button
                            variant="destructive"
                            size="icon"
                            onClick={() => deleteTool(tool.id)}
                          >
                            <Trash2 className="w-4 h-4" />
                          </Button>
                        </div>
                      );
                    })}

                    <Button onClick={addTool} className="w-full">
                      <Plus className="w-4 h-4 mr-2" />
                      Add Tool
                    </Button>
                  </div>
                </CardContent>
              </Card>
            </TabsContent>

            {/* Rack Configuration */}
            <TabsContent value="rack" className="space-y-4">
              <Card>
                <CardHeader>
                  <CardTitle>Tool Rack Configuration</CardTitle>
                  <CardDescription>
                    {config.machine.toolChange === 'automatic'
                      ? 'Assign tools to rack slots for automatic tool change'
                      : 'Automatic tool change is disabled. Enable it in Machine settings.'}
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
                    {config.rackConfig.map(slot => (
                      <div key={slot.slotNumber} className="space-y-3 p-3 border rounded-lg">
                        <div className="flex items-center justify-between">
                          <Label className="font-semibold">Slot {slot.slotNumber}</Label>
                          <div className="flex items-center space-x-2">
                            <Checkbox
                              id={`disable-${slot.slotNumber}`}
                              checked={slot.disabled}
                              onCheckedChange={v => updateRackSlotDisabled(slot.slotNumber, v as boolean)}
                              disabled={config.machine.toolChange === 'manual'}
                            />
                            <Label 
                              htmlFor={`disable-${slot.slotNumber}`} 
                              className="text-sm cursor-pointer"
                            >
                              Disable
                            </Label>
                          </div>
                        </div>
                        
                        <Select
                          value={slot.toolId || 'none'}
                          onValueChange={v =>
                            updateRackSlot(slot.slotNumber, v === 'none' ? null : v)
                          }
                          disabled={config.machine.toolChange === 'manual' || slot.disabled}
                        >
                          <SelectTrigger>
                            <SelectValue placeholder="Select tool..." />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="none">None</SelectItem>
                            {config.toolStock.map(tool => (
                              <SelectItem key={tool.id} value={tool.id}>
                                {tool.name}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                    ))}
                  </div>
                </CardContent>
              </Card>
            </TabsContent>

            {/* Output Configuration */}
            <TabsContent value="output" className="space-y-4">
              <Card>
                <CardHeader>
                  <CardTitle>G-Code Output Templates</CardTitle>
                  <CardDescription>
                    Define templates for each operation type. Use variables in {'{}'} brackets.
                  </CardDescription>
                </CardHeader>
                <CardContent className="space-y-6">
                  {config.outputConfig.operations.map(operation => (
                    <div key={operation.id} className="space-y-2">
                      <div className="flex items-center justify-between">
                        <Label>
                          {operation.id}. {operation.name}
                        </Label>
                        <TooltipProvider>
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <Button variant="ghost" size="icon">
                                <HelpCircle className="w-4 h-4" />
                              </Button>
                            </TooltipTrigger>
                            <TooltipContent className="max-w-xs">
                              <p className="text-sm">
                                Available variables: {Object.keys(VARIABLE_DESCRIPTIONS).join(', ')}
                              </p>
                            </TooltipContent>
                          </Tooltip>
                        </TooltipProvider>
                      </div>
                      <Textarea
                        value={operation.template}
                        onChange={e => updateOperation(operation.id, e.target.value)}
                        className="font-mono text-sm"
                        rows={3}
                      />
                      <div className="flex flex-wrap gap-2">
                        {Object.entries(VARIABLE_DESCRIPTIONS).slice(0, 8).map(([variable, desc]) => (
                          <TooltipProvider key={variable}>
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <Button
                                  variant="outline"
                                  size="sm"
                                  onClick={() => insertVariable(operation.id, variable)}
                                >
                                  {variable}
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent>
                                <p className="text-sm">{desc}</p>
                              </TooltipContent>
                            </Tooltip>
                          </TooltipProvider>
                        ))}
                      </div>
                    </div>
                  ))}
                </CardContent>
              </Card>
            </TabsContent>
          </Tabs>
        </div>
      </div>
    </Layout>
  );
}
