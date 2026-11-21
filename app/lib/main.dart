import 'dart:io';

import 'package:app_links/app_links.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

import 'device_key.dart';
import 'format.dart';
import 'node.dart';
import 'screens/hide_list_screen.dart';
import 'screens/hops_screen.dart';
import 'screens/pair_screen.dart';
import 'screens/welcome_screen.dart';
import 'shell.dart';
import 'src/rust/api/osp.dart';
import 'src/rust/frb_generated.dart';
import 'theme.dart';
import 'uploads.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  LicenseRegistry.addLicense(() async* {
    yield LicenseEntryWithLineBreaks([
      'Inter',
    ], await rootBundle.loadString('fonts/OFL.txt'));
  });
  await RustLib.init();
  HomeNode? node;
  String? error;
  try {
    // App-private storage: the pairing, the saved feed and a post still
    // being sent. The device key is in the phone's secure storage.
    final dir = await getApplicationSupportDirectory();
    final key = await loadDeviceKey(
      const PhoneSecretStore(),
      dir,
      newDeviceKey,
    );
    final uploads = UploadsFolder(
      Directory('${dir.path}/uploads'),
      // Where image_picker leaves its copies: the tmp folder on an iPhone,
      // the cache folder on Android.
      pickerDir: Platform.isIOS
          ? Directory.systemTemp
          : await getTemporaryDirectory(),
    );
    node = await HomeNode.open(dir.path, key, uploads);
  } catch (e) {
    error = describeError(e);
  }
  // An `osp:` link tapped elsewhere, the one that started the app among
  // them (ADR 0039 §4). The shell opens it once the phone is paired.
  if (node case final node?) {
    AppLinks().uriLinkStream.listen(
      (uri) => node.openCode(uri.toString()),
      onError: (Object _) {},
    );
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
      theme: ospTheme(Brightness.light),
      darkTheme: ospTheme(Brightness.dark),
      home: switch (node) {
        null => _StartupError(message: startupError ?? 'unknown error'),
        final node => ListenableBuilder(
          listenable: node,
          builder: (context, _) => node.termsDue
              ? WelcomeScreen(node: node)
              : node.hideListDue
              ? HideListScreen(node: node)
              : node.paired == null
              ? PairScreen(node: node)
              : node.hopsDue
              ? HopsScreen(node: node)
              : AppShell(node: node),
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
