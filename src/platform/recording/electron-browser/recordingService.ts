import { registerMainProcessRemoteService } from '../../ipc/electron-browser/services.js';
import { IRecordingService, RECORDING_CHANNEL_NAME } from '../common/recording.js';

registerMainProcessRemoteService(IRecordingService, RECORDING_CHANNEL_NAME, { proxyOptions: { disableMarshalling: true } });
