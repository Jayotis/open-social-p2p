import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';

import 'device_key.dart';
import 'format.dart';
import 'node.dart';
import 'screens/feed_screen.dart';
import 'screens/pair_screen.dart';
import 'src/rust/api/osp.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  HomeNode? node;
  String? error;
  try {
    // App-private storage: the pairing and the saved feed. The device key
    // is in the phone's secure storage.
    final dir = await getApplicationSupportDirectory();
    final key = await loadDeviceKey(
      const PhoneSecretStore(),
      dir,
      newDeviceKey,
    );
    node = await HomeNode.open(dir.path, key);
  } catch (e) {
    error = describeError(e);
  }
  runApp(PhoneApp(node: node, startupError: error));
}

class PhoneApp extends StatelessWidget {
  const PhoneApp({super.key, required this.node, this.startupError});

  final HomeNode? node;
  final String? startupError;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Open Social P2P',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.teal),
        useMaterial3: true,
      ),
      darkTheme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: Colors.teal,
          brightness: Brightness.dark,
        ),
        useMaterial3: true,
      ),
      home: switch (node) {
        null => _StartupError(message: startupError ?? 'unknown error'),
        final node => ListenableBuilder(
          listenable: node,
          builder: (context, _) => node.paired == null
              ? PairScreen(node: node)
              : FeedScreen(node: node),
        ),
      },
    );
  }
}

class _StartupError extends StatelessWidget {
  const _StartupError({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              const Icon(Icons.error_outline, size: 48),
              const SizedBox(height: 16),
              const Text("The app couldn't start its network connection."),
              const SizedBox(height: 8),
              SelectableText(message),
            ],
          ),
        ),
      ),
    );
  }
}
