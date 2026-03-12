import React from 'react';
import { Layout } from '../components/Layout';
import { useApp } from '../context/AppContext';
import { GCodeViewer } from '../components/GCodeViewer';
import { PCBVisualization } from '../components/PCBVisualization';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../components/ui/card';
import { Button } from '../components/ui/button';
import { Badge } from '../components/ui/badge';
import { Checkbox } from '../components/ui/checkbox';
import { Label } from '../components/ui/label';
import { Alert, AlertDescription } from '../components/ui/alert';
import { RefreshCw, Eye, EyeOff, ArrowRight } from 'lucide-react';
import { useNavigate } from 'react-router';

export function Generator() {
  const navigate = useNavigate();
  const { pcbData, config, gcode, generateGCode, hiddenTools, toggleToolVisibility } = useApp();

  if (!pcbData) {
    return (
      <Layout>
        <div className="p-8">
          <Alert>
            <AlertDescription>
              No PCB data loaded. Please check the Setup page configuration.
            </AlertDescription>
          </Alert>
        </div>
      </Layout>
    );
  }

  return (
    <Layout>
      <div className="p-8">
        <div className="max-w-7xl mx-auto">
          <div className="mb-8 flex items-center justify-between">
            <div>
              <h2 className="text-2xl font-semibold mb-2">G-Code Generator</h2>
              <p className="text-gray-600">
                Visualize and generate tool paths for {pcbData.fileName}
              </p>
            </div>
            <div className="flex gap-3">
              <Button variant="outline" onClick={generateGCode}>
                <RefreshCw className="w-4 h-4 mr-2" />
                Regenerate
              </Button>
              <Button onClick={() => navigate('/output')}>
                Continue to Output
                <ArrowRight className="w-4 h-4 ml-2" />
              </Button>
            </div>
          </div>

          {/* Stats */}
          <div className="grid grid-cols-4 gap-4 mb-6">
            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="text-sm font-medium">Total Operations</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-semibold">
                  {pcbData.drillHoles.length + pcbData.routes.length}
                </div>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="text-sm font-medium">Drill Holes</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-semibold">{pcbData.drillHoles.length}</div>
                <p className="text-xs text-gray-500 mt-1">
                  {pcbData.drillHoles.filter(h => h.type === 'PTH').length} PTH,{' '}
                  {pcbData.drillHoles.filter(h => h.type === 'NPTH').length} NPTH
                </p>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="text-sm font-medium">Routes</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-semibold">{pcbData.routes.length}</div>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="text-sm font-medium">G-Code Lines</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-semibold">{gcode.split('\n').length}</div>
              </CardContent>
            </Card>
          </div>

          <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
            {/* Visualization Panel */}
            <div className="lg:col-span-2 space-y-6">
              <Card>
                <CardHeader>
                  <CardTitle>PCB Visualization</CardTitle>
                  <CardDescription>
                    Visual representation of tool paths and operations
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  <PCBVisualization
                    data={pcbData}
                    tools={config.toolStock}
                    hiddenTools={hiddenTools}
                    width={800}
                    height={500}
                  />
                  <div className="mt-4 flex gap-4 text-sm">
                    <div className="flex items-center gap-2">
                      <div className="w-4 h-4 bg-blue-500 rounded"></div>
                      <span>PTH Holes</span>
                    </div>
                    <div className="flex items-center gap-2">
                      <div className="w-4 h-4 bg-red-500 rounded"></div>
                      <span>NPTH Holes</span>
                    </div>
                    <div className="flex items-center gap-2">
                      <div className="w-4 h-4 bg-yellow-400 rounded"></div>
                      <span>Routes</span>
                    </div>
                    <div className="flex items-center gap-2">
                      <div className="w-4 h-4 bg-green-400 rounded"></div>
                      <span>Board Outline</span>
                    </div>
                  </div>
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle>G-Code Preview</CardTitle>
                  <CardDescription>
                    Generated machine code (read-only in this view)
                  </CardDescription>
                </CardHeader>
                <CardContent className="p-0">
                  <div className="h-96">
                    <GCodeViewer code={gcode} readOnly />
                  </div>
                </CardContent>
              </Card>
            </div>

            {/* Tool Filter Panel */}
            <div className="space-y-6">
              <Card>
                <CardHeader>
                  <CardTitle>Tool Filter</CardTitle>
                  <CardDescription>
                    Show or hide operations by tool
                  </CardDescription>
                </CardHeader>
                <CardContent className="space-y-4">
                  {config.toolStock.map(tool => {
                    const isHidden = hiddenTools.has(tool.id);
                    return (
                      <div key={tool.id} className="flex items-center justify-between">
                        <div className="flex items-center gap-3">
                          <Checkbox
                            id={tool.id}
                            checked={!isHidden}
                            onCheckedChange={() => toggleToolVisibility(tool.id)}
                          />
                          <Label htmlFor={tool.id} className="cursor-pointer">
                            <div>
                              <div className="font-medium">{tool.name}</div>
                              <div className="text-sm text-gray-500">
                                Ø{tool.diameter}mm {tool.type}
                              </div>
                            </div>
                          </Label>
                        </div>
                        {isHidden ? (
                          <EyeOff className="w-4 h-4 text-gray-400" />
                        ) : (
                          <Eye className="w-4 h-4 text-blue-500" />
                        )}
                      </div>
                    );
                  })}
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle>Job Configuration</CardTitle>
                  <CardDescription>Current settings</CardDescription>
                </CardHeader>
                <CardContent className="space-y-3">
                  <div>
                    <Label className="text-sm text-gray-500">Job Type</Label>
                    <p className="font-medium capitalize">{config.jobType.replace(/-/g, ' ')}</p>
                  </div>
                  <div>
                    <Label className="text-sm text-gray-500">Machine</Label>
                    <p className="font-medium">{config.machine.name}</p>
                  </div>
                  <div>
                    <Label className="text-sm text-gray-500">Tool Change</Label>
                    <Badge variant={config.machine.toolChange === 'automatic' ? 'default' : 'secondary'}>
                      {config.machine.toolChange}
                    </Badge>
                  </div>
                  <Button
                    variant="outline"
                    className="w-full mt-4"
                    onClick={() => navigate('/')}
                  >
                    Edit Setup
                  </Button>
                </CardContent>
              </Card>
            </div>
          </div>
        </div>
      </div>
    </Layout>
  );
}