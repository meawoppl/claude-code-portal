/**
 * PCM Audio Processor for Voice Input
 *
 * This AudioWorklet processor converts Float32 audio samples from the microphone
 * to 16-bit PCM (Linear16) format required by Google Speech-to-Text API.
 *
 * Audio spec:
 * - Input: Float32 samples in range [-1, 1] at AudioContext sample rate
 * - Output: Int16 samples in range [-32768, 32767] at 16kHz mono
 */
class PCMProcessor extends AudioWorkletProcessor {
    constructor() {
        super();

        // Buffer to accumulate samples before sending
        // 4096 samples at 16kHz = ~256ms per chunk
        this.bufferSize = 4096;
        this.buffer = new Float32Array(this.bufferSize);
        this.bufferIndex = 0;

        // Resampling state (from AudioContext rate to 16kHz)
        this.inputSampleRate = sampleRate; // AudioWorklet global
        this.outputSampleRate = 16000;
        this.resampleRatio = this.inputSampleRate / this.outputSampleRate;
        this.resampleAccumulator = 0;

        // Track if we're actively recording
        this.isRecording = true;

        // Volume level tracking with smoothing
        this.volumeSampleCount = 0;
        this.volumeSum = 0;
        this.volumeReportInterval = 512; // Report volume every ~32ms at 16kHz (slower updates)
        this.smoothedVolume = 0;
        this.smoothingFactor = 0.3; // Lower = smoother (0.3 means 30% new, 70% old)

        // Silence detection - auto-stop after sustained silence
        this.silenceThreshold = 0.01; // RMS level below which is considered silence
        this.silenceDuration = 0; // How long we've been in silence (in samples)
        this.silenceTimeout = 32000; // ~2 seconds at 16kHz before auto-stop
        this.hasSpokenOnce = false; // Only trigger timeout after speech has started

        // Listen for control messages from main thread
        this.port.onmessage = (event) => {
            if (event.data.command === 'stop') {
                this.isRecording = false;
                // Flush any remaining buffered audio
                if (this.bufferIndex > 0) {
                    this.flushBuffer();
                }
            } else if (event.data.command === 'start') {
                this.isRecording = true;
                this.bufferIndex = 0;
            }
        };
    }

    /**
     * Convert accumulated Float32 buffer to Int16 and send to main thread
     */
    flushBuffer() {
        const samplesToSend = this.bufferIndex;
        if (samplesToSend === 0) return;

        // Convert Float32 [-1, 1] to Int16 [-32768, 32767]
        const pcm16 = new Int16Array(samplesToSend);
        for (let i = 0; i < samplesToSend; i++) {
            // Clamp and scale
            const sample = Math.max(-1, Math.min(1, this.buffer[i]));
            pcm16[i] = Math.floor(sample * 32767);
        }

        // Send as transferable ArrayBuffer for zero-copy
        this.port.postMessage(
            { audioData: pcm16.buffer, samples: samplesToSend },
            [pcm16.buffer]
        );

        // Reset buffer
        this.buffer = new Float32Array(this.bufferSize);
        this.bufferIndex = 0;
    }

    /**
     * Process audio samples from the microphone
     * Called ~every 128 samples by the audio worklet system
     */
    process(inputs, outputs, parameters) {
        if (!this.isRecording) {
            return true; // Keep processor alive but don't process
        }

        const input = inputs[0];
        if (!input || !input[0]) {
            return true; // No input, keep alive
        }

        const inputChannel = input[0]; // Mono - first channel only

        // Simple resampling: skip samples to downsample from input rate to 16kHz
        // For better quality, a proper resampling filter could be used
        for (let i = 0; i < inputChannel.length; i++) {
            this.resampleAccumulator += 1;

            // Take a sample when we've accumulated enough input samples
            if (this.resampleAccumulator >= this.resampleRatio) {
                this.resampleAccumulator -= this.resampleRatio;

                const sample = inputChannel[i];
                this.buffer[this.bufferIndex++] = sample;

                // Track volume (RMS)
                this.volumeSum += sample * sample;
                this.volumeSampleCount++;

                // Report volume at regular intervals with smoothing
                if (this.volumeSampleCount >= this.volumeReportInterval) {
                    const rms = Math.sqrt(this.volumeSum / this.volumeSampleCount);
                    // Convert to 0-1 range with some amplification for better visual feedback
                    const rawLevel = Math.min(1.0, rms * 3);
                    // Apply exponential smoothing for less frantic display
                    this.smoothedVolume = this.smoothingFactor * rawLevel + (1 - this.smoothingFactor) * this.smoothedVolume;
                    this.port.postMessage({ volumeLevel: this.smoothedVolume });

                    // Silence detection
                    if (rms > this.silenceThreshold) {
                        // Sound detected - mark that speech has started and reset silence counter
                        this.hasSpokenOnce = true;
                        this.silenceDuration = 0;
                    } else if (this.hasSpokenOnce) {
                        // In silence after speech - accumulate silence duration
                        this.silenceDuration += this.volumeSampleCount;

                        // Check if we've exceeded the silence timeout
                        if (this.silenceDuration >= this.silenceTimeout) {
                            // Signal auto-stop due to silence
                            this.port.postMessage({ silenceDetected: true });
                            this.silenceDuration = 0; // Reset to avoid repeated signals
                        }
                    }

                    this.volumeSum = 0;
                    this.volumeSampleCount = 0;
                }

                // Buffer full - send to main thread
                if (this.bufferIndex >= this.bufferSize) {
                    this.flushBuffer();
                }
            }
        }

        return true; // Keep processor alive
    }
}

registerProcessor('pcm-processor', PCMProcessor);
