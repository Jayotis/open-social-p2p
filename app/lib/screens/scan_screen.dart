import 'package:flutter/material.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

/// Scans QR codes until one passes [accept], then returns its text.
class ScanScreen extends StatefulWidget {
  const ScanScreen({
    super.key,
    required this.title,
    required this.hint,
    required this.accept,
    required this.wrongCode,
  });

  final String title;
  final String hint;
  final bool Function(String text) accept;

  /// Shown when a QR code is found but isn't the kind wanted.
  final String wrongCode;

  @override
  State<ScanScreen> createState() => _ScanScreenState();
}

class _ScanScreenState extends State<ScanScreen> {
  final _controller = MobileScannerController(formats: [BarcodeFormat.qrCode]);
  bool _done = false;
  String? _lastWrong;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _onDetect(BarcodeCapture capture) {
    if (_done) return;
    for (final code in capture.barcodes) {
      final text = code.rawValue;
      if (text == null) continue;
      if (widget.accept(text)) {
        _done = true;
        Navigator.of(context).pop(text);
        return;
      }
      if (text != _lastWrong) {
        _lastWrong = text;
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text(widget.wrongCode)));
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: Text(widget.title)),
      body: Stack(
        children: [
          MobileScanner(
            controller: _controller,
            onDetect: _onDetect,
            errorBuilder: (context, error) => Center(
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Text(
                  "The camera isn't available (${error.errorCode.name}). "
                  'You can paste the code instead.',
                  textAlign: TextAlign.center,
                ),
              ),
            ),
          ),
          Align(
            alignment: Alignment.bottomCenter,
            child: Container(
              width: double.infinity,
              color: Colors.black54,
              padding: const EdgeInsets.all(16),
              child: Text(
                widget.hint,
                textAlign: TextAlign.center,
                style: const TextStyle(color: Colors.white),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Asks for a code to be pasted, for when scanning isn't possible.
Future<String?> askForCode(BuildContext context, String title) {
  final controller = TextEditingController();
  return showDialog<String>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(title),
      content: TextField(
        controller: controller,
        autofocus: true,
        minLines: 3,
        maxLines: 6,
        decoration: const InputDecoration(border: OutlineInputBorder()),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, controller.text.trim()),
          child: const Text('OK'),
        ),
      ],
    ),
  );
}
