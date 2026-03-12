import React, { useState } from 'react';
import { Layout } from '../components/Layout';
import { useApp } from '../context/AppContext';
import { GCodeViewer } from '../components/GCodeViewer';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../components/ui/card';
import { Button } from '../components/ui/button';
import { Alert, AlertDescription } from '../components/ui/alert';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '../components/ui/dialog';
import { Input } from '../components/ui/input';
import { Label } from '../components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import { Progress } from '../components/ui/progress';
import { Save, Usb, Send, Disc, AlertCircle } from 'lucide-react';
import { toast } from 'sonner';

export function Output() {
  const { pcbData, gcode, setGcode } = useApp();
  const [saveDialogOpen, setSaveDialogOpen] = useState(false);
  const [removableDialogOpen, setRemovableDialogOpen] = useState(false);
  const [sendDialogOpen, setSendDialogOpen] = useState(false);
  const [fileName, setFileName] = useState('output.nc');
  const [selectedDrive, setSelectedDrive] = useState('');
  const [ipAddress, setIpAddress] = useState('192.168.1.100');
  const [port, setPort] = useState('8080');
  const [uploading, setUploading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState(0);
  const [isSaved, setIsSaved] = useState(false);

  // Mock removable drives
  const mockDrives = ['E: (USB Drive)', 'F: (SD Card)', 'G: (External HDD)'];

  const handleSaveToFile = () => {
    try {
      const blob = new Blob([gcode], { type: 'text/plain' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = fileName;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      
      setIsSaved(true);
      toast.success(`File saved as ${fileName}`);
      setSaveDialogOpen(false);
    } catch (error) {
      toast.error('Failed to save file');
    }
  };

  const handleSaveToRemovable = () => {
    if (!selectedDrive) {
      toast.error('Please select a drive');
      return;
    }

    // Simulate file transfer
    setUploading(true);
    setUploadProgress(0);

    const interval = setInterval(() => {
      setUploadProgress(prev => {
        if (prev >= 100) {
          clearInterval(interval);
          setUploading(false);
          setIsSaved(true);
          toast.success(`File saved to ${selectedDrive}`);
          setRemovableDialogOpen(false);
          return 100;
        }
        return prev + 10;
      });
    }, 200);
  };

  const handleEjectDrive = () => {
    if (!selectedDrive) {
      toast.error('Please select a drive to eject');
      return;
    }
    toast.success(`${selectedDrive} can now be safely removed`);
  };

  const handleSendOverAir = () => {
    if (!ipAddress || !port) {
      toast.error('Please enter IP address and port');
      return;
    }

    // Simulate network transfer
    setUploading(true);
    setUploadProgress(0);

    const interval = setInterval(() => {
      setUploadProgress(prev => {
        if (prev >= 100) {
          clearInterval(interval);
          setUploading(false);
          toast.success(`G-Code sent to ${ipAddress}:${port}`);
          setSendDialogOpen(false);
          return 100;
        }
        return prev + 8;
      });
    }, 250);
  };

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

  if (!gcode) {
    return (
      <Layout>
        <div className="p-8">
          <Alert>
            <AlertCircle className="h-4 w-4" />
            <AlertDescription>
              No G-Code generated yet. Please visit the Generator page first.
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
          <div className="mb-8">
            <h2 className="text-2xl font-semibold mb-2">Output & Export</h2>
            <p className="text-gray-600">
              Review, edit, and export your G-Code
            </p>
          </div>

          <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
            {/* G-Code Editor */}
            <div className="lg:col-span-2">
              <Card>
                <CardHeader>
                  <CardTitle>G-Code Editor</CardTitle>
                  <CardDescription>
                    Review and make manual adjustments to the generated code
                  </CardDescription>
                </CardHeader>
                <CardContent className="p-0">
                  <div className="h-[600px]">
                    <GCodeViewer code={gcode} onChange={setGcode} />
                  </div>
                </CardContent>
              </Card>
            </div>

            {/* Export Options */}
            <div className="space-y-6">
              <Card>
                <CardHeader>
                  <CardTitle>Export Options</CardTitle>
                  <CardDescription>
                    Save or send your G-Code
                  </CardDescription>
                </CardHeader>
                <CardContent className="space-y-3">
                  <Button
                    className="w-full"
                    onClick={() => setSaveDialogOpen(true)}
                  >
                    <Save className="w-4 h-4 mr-2" />
                    Save to File
                  </Button>

                  <Button
                    className="w-full"
                    variant="outline"
                    onClick={() => setRemovableDialogOpen(true)}
                  >
                    <Usb className="w-4 h-4 mr-2" />
                    Save to Removable Media
                  </Button>

                  <Button
                    className="w-full"
                    variant="outline"
                    onClick={() => setSendDialogOpen(true)}
                  >
                    <Send className="w-4 h-4 mr-2" />
                    Send Over the Air
                  </Button>

                  {isSaved && selectedDrive && (
                    <Button
                      className="w-full"
                      variant="secondary"
                      onClick={handleEjectDrive}
                    >
                      <Disc className="w-4 h-4 mr-2" />
                      Eject {selectedDrive.split(' ')[0]}
                    </Button>
                  )}
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle>File Information</CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  <div>
                    <Label className="text-sm text-gray-500">Source File</Label>
                    <p className="font-medium">{pcbData.fileName}</p>
                  </div>
                  <div>
                    <Label className="text-sm text-gray-500">G-Code Size</Label>
                    <p className="font-medium">
                      {(new Blob([gcode]).size / 1024).toFixed(2)} KB
                    </p>
                  </div>
                  <div>
                    <Label className="text-sm text-gray-500">Lines of Code</Label>
                    <p className="font-medium">{gcode.split('\n').length}</p>
                  </div>
                  <div>
                    <Label className="text-sm text-gray-500">Status</Label>
                    <p className="font-medium text-green-600">
                      {isSaved ? '✓ Saved' : 'Not saved'}
                    </p>
                  </div>
                </CardContent>
              </Card>
            </div>
          </div>
        </div>
      </div>

      {/* Save to File Dialog */}
      <Dialog open={saveDialogOpen} onOpenChange={setSaveDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Save G-Code to File</DialogTitle>
            <DialogDescription>
              Enter a filename for your G-Code output
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 py-4">
            <div className="space-y-2">
              <Label>File Name</Label>
              <Input
                value={fileName}
                onChange={e => setFileName(e.target.value)}
                placeholder="output.nc"
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setSaveDialogOpen(false)}>
              Cancel
            </Button>
            <Button onClick={handleSaveToFile}>
              <Save className="w-4 h-4 mr-2" />
              Save
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Save to Removable Media Dialog */}
      <Dialog open={removableDialogOpen} onOpenChange={setRemovableDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Save to Removable Media</DialogTitle>
            <DialogDescription>
              Select a drive and save your G-Code
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 py-4">
            <div className="space-y-2">
              <Label>File Name</Label>
              <Input
                value={fileName}
                onChange={e => setFileName(e.target.value)}
                placeholder="output.nc"
              />
            </div>
            <div className="space-y-2">
              <Label>Select Drive</Label>
              <Select value={selectedDrive} onValueChange={setSelectedDrive}>
                <SelectTrigger>
                  <SelectValue placeholder="Choose a drive..." />
                </SelectTrigger>
                <SelectContent>
                  {mockDrives.map(drive => (
                    <SelectItem key={drive} value={drive}>
                      {drive}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            {uploading && (
              <div className="space-y-2">
                <Label>Progress</Label>
                <Progress value={uploadProgress} />
                <p className="text-sm text-gray-500">{uploadProgress}% complete</p>
              </div>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setRemovableDialogOpen(false)}
              disabled={uploading}
            >
              Cancel
            </Button>
            <Button onClick={handleSaveToRemovable} disabled={uploading}>
              <Usb className="w-4 h-4 mr-2" />
              Save to Drive
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Send Over the Air Dialog */}
      <Dialog open={sendDialogOpen} onOpenChange={setSendDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Send Over the Air</DialogTitle>
            <DialogDescription>
              Transfer G-Code directly to your CNC machine via network
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 py-4">
            <div className="space-y-2">
              <Label>CNC Machine IP Address</Label>
              <Input
                value={ipAddress}
                onChange={e => setIpAddress(e.target.value)}
                placeholder="192.168.1.100"
              />
            </div>
            <div className="space-y-2">
              <Label>Port</Label>
              <Input
                value={port}
                onChange={e => setPort(e.target.value)}
                placeholder="8080"
              />
            </div>
            {uploading && (
              <div className="space-y-2">
                <Label>Sending...</Label>
                <Progress value={uploadProgress} />
                <p className="text-sm text-gray-500">{uploadProgress}% complete</p>
              </div>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setSendDialogOpen(false)}
              disabled={uploading}
            >
              Cancel
            </Button>
            <Button onClick={handleSendOverAir} disabled={uploading}>
              <Send className="w-4 h-4 mr-2" />
              Send
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Layout>
  );
}