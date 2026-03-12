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
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '../components/ui/tooltip';
import { Plus, Trash2, HelpCircle } from 'lucide-react';
import { toast } from 'sonner';
import type { Tool, RackSlot, OperationTemplate } from '../context/AppContext';

export function Configuration() {
  const { config, updateConfig } = useApp();
  const [activeTab, setActiveTab] = useState('job');

  const updateJobType = (jobType: string) => {
    updateConfig({ jobType: jobType as any });
    toast.success('Job type updated');
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

  return (
    <Layout>
      <div className="p-8">
        <div className="max-w-6xl mx-auto">
          <div className="mb-8">
            <h2 className="text-2xl font-semibold mb-2">Configuration</h2>
            <p className="text-gray-600">
              Manage all settings for G-Code generation
            </p>
          </div>

          <Tabs value={activeTab} onValueChange={setActiveTab}>
            <TabsList className="grid w-full grid-cols-5">
              <TabsTrigger value="job">Job Type</TabsTrigger>
              <TabsTrigger value="machine">Machine</TabsTrigger>
              <TabsTrigger value="tools">Tool Stock</TabsTrigger>
              <TabsTrigger value="rack">Rack Config</TabsTrigger>
              <TabsTrigger value="output">Output</TabsTrigger>
            </TabsList>

            {/* Job Type Configuration */}
            <TabsContent value="job" className="space-y-4">
              <Card>
                <CardHeader>
                  <CardTitle>Production Job Type</CardTitle>
                  <CardDescription>
                    Select the type of manufacturing operation
                  </CardDescription>
                </CardHeader>
                <CardContent className="space-y-4">
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                    {[
                      { value: 'drill-locating', label: 'Drill Locating Pins', desc: 'Create alignment holes' },
                      { value: 'drill-pth', label: 'Drill PTH Holes', desc: 'Plated through holes only' },
                      { value: 'drill-npth-route', label: 'Drill NPTH and Route', desc: 'Non-plated holes and routing' },
                      { value: 'route-board', label: 'Route Board', desc: 'Board outline routing' },
                    ].map(job => (
                      <Card
                        key={job.value}
                        className={`cursor-pointer transition-all ${
                          config.jobType === job.value
                            ? 'border-blue-500 bg-blue-50'
                            : 'hover:border-gray-400'
                        }`}
                        onClick={() => updateJobType(job.value)}
                      >
                        <CardHeader>
                          <CardTitle className="text-base">{job.label}</CardTitle>
                          <CardDescription className="text-sm">{job.desc}</CardDescription>
                        </CardHeader>
                      </Card>
                    ))}
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
                  <CardTitle>Tool Stock</CardTitle>
                  <CardDescription>
                    Manage available drill bits and router bits
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="space-y-4">
                    {config.toolStock.map(tool => (
                      <div key={tool.id} className="flex gap-4 items-end p-4 border rounded-lg">
                        <div className="flex-1 grid grid-cols-4 gap-4">
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
                            <Label>Diameter (mm)</Label>
                            <Input
                              type="number"
                              step="0.1"
                              value={tool.diameter}
                              onChange={e =>
                                updateTool(tool.id, { diameter: parseFloat(e.target.value) })
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
                        </div>

                        <Button
                          variant="destructive"
                          size="icon"
                          onClick={() => deleteTool(tool.id)}
                        >
                          <Trash2 className="w-4 h-4" />
                        </Button>
                      </div>
                    ))}

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
                      <div key={slot.slotNumber} className="space-y-2">
                        <Label>Slot {slot.slotNumber}</Label>
                        <Select
                          value={slot.toolId || 'none'}
                          onValueChange={v =>
                            updateRackSlot(slot.slotNumber, v === 'none' ? null : v)
                          }
                          disabled={config.machine.toolChange === 'manual'}
                        >
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="none">Do Not Use</SelectItem>
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
